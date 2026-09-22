import test from "node:test";
import assert from "node:assert/strict";
import { mkdir, rm, readFile, writeFile, readdir } from "node:fs/promises";
import { execFile } from "node:child_process";
import { promisify } from "node:util";
import { resolve } from "node:path";
import { createServer } from "vite";
import { chromium } from "playwright";
import { anthropicReply } from "./provider-fixture.mjs";

const exec = promisify(execFile);
const work = resolve(".work/regression");
const repo = resolve("../../..");

test("WebCM regressions run actual browser modules and local synthetic shell inputs", { timeout: 120_000 }, async t => {
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
    let externalRequests = 0;
    await page.route("**/*", route => {
      if (!route.request().url().startsWith(origin + "/")) {
        externalRequests++;
        return route.abort();
      }
      if (route.request().url() === origin + "/") return route.fulfill({ contentType: "text/html", body: "<div id='app'></div>" });
      return route.continue();
    });
    await page.goto(origin);
    await page.exposeFunction("syntheticShell", async command => {
      try {
        const result = await exec("/bin/sh", ["-c", command], { cwd: work, timeout: 5000, maxBuffer: 1024 * 1024 });
        return { output: result.stdout + result.stderr, exitCode: 0 };
      } catch (error) {
        if (typeof error.code !== "number") throw error;
        return { output: (error.stdout ?? "") + (error.stderr ?? ""), exitCode: error.code };
      }
    });
    await t.test("E07-E09 shell framing, exact file bytes, literal names, write failures and queue recovery", async () => {
      const result = await page.evaluate(async () => {
        const { WebCMHost, frameCommand, parseCommandOutput } = await import("/src/webcm-host.ts");
        const { registerWebCMTools } = await import("/src/tools.ts");
        const equal = (a, b) => { if (JSON.stringify(a) !== JSON.stringify(b)) throw new Error(`${JSON.stringify(a)} != ${JSON.stringify(b)}`); };
        const fixtures = [
          ["true", "", 0], ["printf abc", "abc", 0], ["printf '  abc\\n\\n '", "  abc\n\n ", 0],
          ["printf first\nprintf '\\nsecond' # trailing comment", "first\nsecond", 0],
          ["printf bad; false # exit status", "bad", 1],
        ];
        for (const [command, output, exitCode] of fixtures) {
          const marker = "__SYNTHETIC__";
          const raw = await window.syntheticShell(frameCommand(command, marker));
          equal(parseCommandOutput("echoed source\n" + raw.output + "prompt", marker), { output, exitCode });
        }
        const framedHost = new WebCMHost();
        framedHost.booted = true;
        framedHost.writeToShell = text => {
          window.syntheticShell(text).then(result => { framedHost.outputBuffer = "echoed source\n" + result.output; });
        };
        for (const [command, output, exitCode] of fixtures) equal(await framedHost.exec(command, 5000), { output, exitCode });
        const host = new WebCMHost();
        host.exec = window.syntheticShell;
        for (const name of ["space name", "apostrophe'name", "$cash*", "-dash"]) {
          for (const content of [" a\r\nb\n  ", "Unicode é 🐾\n".repeat(100)]) {
            equal((await host.writeFile(name, content)).exitCode, 0);
            equal(await host.readFile(name), content);
          }
        }
        const callbacks = {};
        const runtime = { clear_tool_callbacks() {}, register_tool_callback(name, _d, _s, callback) { callbacks[name] = callback; } };
        let writes = 0;
        let mode = "directory";
        registerWebCMTools(runtime, {
          async exec() { if (mode === "reject") throw Error("rejected command"); return { output: "mkdir diagnostic", exitCode: mode === "directory" ? 7 : 0 }; },
          async writeFile() { writes++; return { output: "write diagnostic", exitCode: mode === "write" ? 8 : 0 }; },
          async readFile() { return "literal"; },
        });
        let response = JSON.parse(await callbacks.write_file('{"path":"a b/file","content":"x"}'));
        equal(response, { content: "Exit code 7\nmkdir diagnostic", is_error: true }); equal(writes, 0);
        mode = "write";
        response = JSON.parse(await callbacks.write_file('{"path":"a b/file","content":"x"}'));
        equal(response, { content: "Exit code 8\nwrite diagnostic", is_error: true });
        mode = "reject";
        await callbacks.shell('{"command":"true"}').then(() => { throw Error("expected rejection"); }, () => {});
        mode = "ok";
        equal(JSON.parse(await callbacks.write_file('{"path":"a b/file","content":"x"}')).is_error, false);
        const chunks = new WebCMHost();
        const commands = [];
        chunks.exec = async command => {
          commands.push(command);
          return { output: "chunk failure", exitCode: commands.length === 3 ? 9 : 0 };
        };
        equal(await chunks.writeFile("large file", "x".repeat(2000)), { output: "chunk failure", exitCode: 9 });
        equal(commands.length, 4);
        if (!commands[3].startsWith("rm -f -- ") || commands.some(c => c.startsWith("base64 -d"))) throw Error("did not stop at failed chunk");
        const short = new WebCMHost();
        short.exec = async () => ({ output: "short failure", exitCode: 10 });
        equal(await short.writeFile("small", "x"), { output: "short failure", exitCode: 10 });
        return fixtures.length;
      });
      assert.equal(result, 5);
      assert.deepEqual((await readdir(work)).filter(n => ["space name", "apostrophe'name", "$cash*", "-dash"].includes(n)).sort(), ["$cash*", "-dash", "apostrophe'name", "space name"]);
    });

    await t.test("E02/E10/E11 typed events and independent, idempotent tool cards", async () => {
      await page.evaluate(async () => {
        const { MobOrchestrator } = await import("/src/mob.ts");
        const { StreamRenderer } = await import("/src/stream.ts");
        const container = document.createElement("div"); document.body.append(container);
        const panel = { stream: new StreamRenderer(container), statusEl: document.createElement("div"), pendingCards: new Map(), seenEvents: new Set(), subHandle: null };
        const mob = new MobOrchestrator({}, new Map([["planner", panel]]));
        let seq = 0;
        const send = payload => mob.routeEnvelope("planner", { event_id: `event-${++seq}`, payload }, panel);
        const check = (value, message) => { if (!value) throw Error(message); };
        send({ type: "run_started", input: { kind: "content", content: "scalar input" } });
        send({ type: "run_started", input: { kind: "content", content: [{ type: "text", text: "block input" }, { type: "image", source: {} }] } });
        const before = container.textContent;
        send({ type: "run_started", input: { kind: "pending_tool_results" } });
        check(container.textContent === before, "pending continuation fabricated a prompt");
        check(before.includes("scalar input") && before.includes("block input"), "typed prompt omitted");
        for (const id of ["a", "b"]) send({ type: "tool_call_requested", id, name: "shell", args: { command: id } });
        const a = panel.pendingCards.get("a"), b = panel.pendingCards.get("b");
        send({ type: "tool_execution_completed", id: "b", content: [{ type: "text", text: "result b" }, { type: "image", source: {} }], is_error: false });
        send({ type: "tool_result_received", id: "a", content: [{ type: "text", text: "result a" }], is_error: true });
        send({ type: "tool_result_received", id: "b", content: [{ type: "text", text: "must not overwrite" }], is_error: false });
        check(a.textContent.includes("result a") && b.textContent.includes("result b"), "cross-assigned results");
        check(!container.querySelector(".stream-tool-spinner") && !panel.pendingCards.size, "spinner left");
        const replay = { event_id: "replayed", payload: { type: "tool_call_requested", id: "a", name: "read_file" } };
        mob.routeEnvelope("planner", replay, panel); mob.routeEnvelope("planner", replay, panel);
        check(panel.pendingCards.size === 1, "replayed request duplicated");
        send({ type: "tool_execution_timed_out", id: "a", timeout_ms: 10 });
        send({ type: "tool_call_requested", id: "c", name: "shell" });
        send({ type: "run_failed", error_report: { class: "llm", message: "synthetic auth diagnostic" } });
        check(!container.querySelector(".stream-tool-spinner"), "failure left spinner");
        check(container.textContent.includes("synthetic auth diagnostic") && container.textContent.includes("Timed out after 10ms"), "lost diagnostic");
        check(JSON.parse(container.querySelector(".stream-error").dataset.errorReport).class === "llm", "typed failure report discarded");
        send({ type: "tool_call_requested", id: "d", name: "shell" });
        send({ type: "run_completed" });
        check(container.textContent.includes("without a tool result"), "termination fabricated success");
        // Reverse completion order, including per-call error.
        for (const id of ["e", "f"]) send({ type: "tool_call_requested", id, name: "shell" });
        send({ type: "tool_execution_completed", id: "e", content: [{ type: "text", text: "e first" }], is_error: true });
        send({ type: "tool_execution_completed", id: "f", content: [{ type: "text", text: "f last" }], is_error: false });
        check(!panel.pendingCards.size, "reverse order left cards");
      });
    });

    await t.test("E12 safe fresh/pending Markdown, links, highlighted code and model labels", async () => {
      await page.evaluate(async () => {
        const { StreamRenderer } = await import("/src/stream.ts");
        const el = document.createElement("div"); document.body.append(el);
        const stream = new StreamRenderer(el);
        window.executed = 0;
        const bad = '<img src=x onerror="window.executed++"><a href="javascript:window.executed++">unsafe</a>\n\n[bad](javascript:alert%281%29)\n\n**normal**\n\n```js\nconst x = 1;\n```';
        stream.finalizeText(bad);
        stream.appendTextDelta("pending"); stream.finalizeText(bad);
        stream.appendBanner({ main: '<img src=x onerror="window.executed++">', planner: "p", coder: "c", reviewer: "r" });
        await new Promise(r => setTimeout(r, 50));
        if (window.executed || el.querySelector("[onerror]") || [...el.querySelectorAll("a")].some(a => a.href.startsWith("javascript:"))) throw Error("unsafe HTML retained");
        if (el.querySelectorAll("strong").length !== 2 || el.querySelectorAll("pre code.hljs").length !== 2) throw Error("normal markdown lost");
        if (el.querySelector(".model-tag").textContent !== '<img src=x onerror="window.executed++">' || el.querySelector(".model-tag img")) throw Error("model label not literal");
      });
    });

    await t.test("E13/E14 readonly flags, repeated failed boot cleanup and concurrent ownership", async () => {
      await page.route("**/webcm.mjs", route => route.fulfill({ status: 404, body: "missing synthetic VM" }));
      await page.evaluate(async () => {
        const { WebCMHost, rawTermios } = await import("/src/webcm-host.ts");
        const original = Object.freeze({ iflag: 0xffff, cflag: 0xffff, lflag: 0xffff, oflag: 0, cc: Object.freeze([1, 2]) });
        const flags = rawTermios(original);
        if (flags === original || original.iflag !== 0xffff || flags.iflag !== (0xffff & ~0x5eb) || flags.cflag !== ((0xffff & ~0x130) | 0x30) || flags.lflag !== (0xffff & ~0x804b) || flags.oflag !== 1) throw Error("incorrect raw flags");
        const host = new WebCMHost(), container = document.createElement("div"); document.body.append(container);
        for (let i = 0; i < 3; i++) {
          const first = host.boot(container, () => {});
          const second = host.boot(container, () => {});
          if (first !== second) throw Error("concurrent boots did not share promise");
          await first.then(() => { throw Error("missing module unexpectedly booted"); }, () => {});
          if (container.querySelector(".xterm") || host.outputListener !== null || host.slave !== null || host.isBooted()) throw Error("failed boot leaked resources");
        }
      });
      const success = await browser.newPage();
      await success.route("**/*", route => {
        const url = route.request().url();
        if (!url.startsWith(origin + "/")) return route.abort();
        if (url.endsWith("/webcm.mjs")) return route.fulfill({
          contentType: "text/javascript",
          body: "export default async ({pty}) => { globalThis.syntheticBoots = (globalThis.syntheticBoots || 0) + 1; pty.write('$ '); };",
        });
        if (url === origin + "/") return route.fulfill({ contentType: "text/html", body: "<div id='terminal' style='width:500px;height:300px'></div>" });
        return route.continue();
      });
      await success.goto(origin);
      await success.evaluate(async () => {
        const { WebCMHost } = await import("/src/webcm-host.ts");
        const host = new WebCMHost(), container = document.getElementById("terminal");
        await Promise.all([host.boot(container, () => {}), host.boot(container, () => {})]);
        const owner = host.terminal;
        await host.boot(container, () => {});
        if (!host.isBooted() || host.terminal !== owner || window.syntheticBoots !== 1 || container.querySelectorAll(".xterm").length !== 1) throw Error("boot owner not reused");
      });
      await success.close();
    });

    await t.test("E01/E19 current 0.8.40 WASM accepts all four real definitions offline", async () => {
      const result = await page.evaluate(async repoPath => {
        const wasm = await import(`/@fs/${repoPath}/sdks/web/wasm/meerkat_web_runtime.js`);
        const bytes = await (await fetch(`/@fs/${repoPath}/sdks/web/wasm/meerkat_web_runtime_bg.wasm`)).arrayBuffer();
        await wasm.default({ module_or_path: bytes });
        const { buildMobDefinition, resolveModels } = await import("/src/mob.ts");
        let accepted = 0, legacyRejected = 0;
        for (const keys of [{ anthropic: "unused" }, { openai: "unused" }, { gemini: "unused" }, { anthropic: "unused", openai: "unused", gemini: "unused" }]) {
          wasm.init_runtime_from_config(JSON.stringify({ anthropic_api_key: "unused", openai_api_key: "unused", gemini_api_key: "unused" }));
          const def = buildMobDefinition(resolveModels(keys));
          def.id = `offline-${accepted}`;
          for (const skill of Object.values(def.skills)) {
            if (skill.content.includes("send_message(to:") || !skill.content.includes("peer_id") || !skill.content.includes('handling_mode: "queue"')) throw Error("obsolete addressing");
          }
          const legacy = structuredClone(def);
          for (const profile of Object.values(legacy.profiles)) profile.provider_params = { reasoning_effort: "low" };
          await wasm.mob_create(JSON.stringify(legacy)).then(() => { throw Error("legacy should fail"); }, () => { legacyRejected++; });
          await wasm.mob_create(JSON.stringify(def)); accepted++;
          wasm.destroy_runtime();
        }
        return { version: wasm.runtime_version(), accepted, legacyRejected };
      }, repo);
      assert.deepEqual(result, { version: "0.8.40", accepted: 4, legacyRejected: 4 });
    });

    await t.test("real WASM MobOrchestrator startup, all six wires, subscriptions and successful typed events", { timeout: 45_000 }, async () => {
      let requests = 0;
      await page.route("**/fixtures/anthropic/**", route => {
        if (++requests > 40) return route.abort();
        return route.fulfill(anthropicReply(JSON.parse(route.request().postData()), requests, "Synthetic specialist ready."));
      });
      const control = await page.evaluate(async ({ repoPath, origin }) => {
        const wasm = await import(`/@fs/${repoPath}/sdks/web/wasm/meerkat_web_runtime.js`);
        wasm.init_runtime_from_config(JSON.stringify({ anthropic_api_key: "synthetic-unused", anthropic_base_url: `${origin}/fixtures/anthropic`, model: "claude-sonnet-4-6" }));
        let sub;
        try {
          const id = await wasm.mob_create(JSON.stringify({
            id: "offline-control", profiles: { worker: { model: "claude-sonnet-4-6", runtime_mode: "autonomous_host", tools: { comms: true }, external_addressable: true } },
          }));
          await wasm.mob_spawn(id, JSON.stringify([{ profile: "worker", agent_identity: "worker", runtime_mode: "autonomous_host" }]));
          sub = await wasm.mob_member_subscribe(id, "worker");
          await wasm.mob_member_send(id, "worker", JSON.stringify({ content: "Synthetic readiness check.", handling_mode: "queue" }));
          const payloads = [];
          for (let i = 0; i < 200; i++) {
            await new Promise(r => setTimeout(r, 20));
            payloads.push(...JSON.parse(wasm.poll_subscription(sub)).map(e => e.payload));
            if (payloads.some(p => p.type === "run_completed" || p.type === "run_failed")) break;
          }
          return payloads.filter(p => p.type === "run_completed" || p.type === "run_failed");
        } finally {
          if (sub) wasm.close_subscription(sub);
          wasm.destroy_runtime();
        }
      }, { repoPath: repo, origin });
      assert.ok(control.some(p => p.type === "run_completed" && p.result.includes("Synthetic specialist ready.")), `successful fixture control: ${JSON.stringify(control)}`);
      assert.ok(control.every(p => p.type !== "run_failed"), JSON.stringify(control));
      const controlRequests = requests;
      const result = await page.evaluate(async ({ repoPath, origin }) => {
        const wasm = await import(`/@fs/${repoPath}/sdks/web/wasm/meerkat_web_runtime.js`);
        const { MobOrchestrator, resolveModels } = await import("/src/mob.ts");
        const { StreamRenderer } = await import("/src/stream.ts");
        const agents = ["orchestrator", "planner", "coder", "reviewer"];
        const panels = new Map(agents.map(agent => {
          const el = document.createElement("div"); document.body.append(el);
          return [agent, { stream: new StreamRenderer(el), statusEl: document.createElement("div"), pendingCards: new Map(), seenEvents: new Set(), subHandle: null, element: el }];
        }));
        const stages = [];
        const runtime = { ...wasm };
        for (const name of ["mob_create", "mob_spawn", "mob_wire", "mob_member_subscribe"]) {
          runtime[name] = async (...args) => {
            stages.push({ name, phase: "start", member: name === "mob_member_subscribe" ? args[1] : undefined });
            try { const result = await wasm[name](...args); stages.push({ name, phase: "done" }); return result; }
            catch (error) { stages.push({ name, phase: "failed", error: String(error) }); throw error; }
          };
        }
        const mob = new MobOrchestrator(runtime, panels);
        try {
          const keys = { anthropic: "synthetic-unused" };
          await Promise.race([
            mob.init(keys, resolveModels(keys), `${origin}/fixtures`),
            new Promise((_, reject) => setTimeout(() => reject(Error(`Actual startup deadline: ${JSON.stringify(stages)}`)), 35_000)),
          ]);
          // Assert wire success directly: the production initializer tolerates
          // duplicate-wire errors, so its completion alone is not evidence.
          for (let i = 0; i < agents.length; i++) {
            for (const peer of agents.slice(i + 1)) await wasm.mob_wire(mob.mobId, agents[i], peer);
          }
          if ([...panels.values()].some(panel => !panel.subHandle)) throw Error("missing member subscription");
          for (const agent of agents) {
            const target = JSON.parse(await wasm.mob_member_peer_target(mob.mobId, agent));
            if (typeof target.external?.peer_id !== "string") throw Error("missing peer identity");
            await wasm.mob_member_send(mob.mobId, agent, JSON.stringify({ content: "Synthetic readiness check.", handling_mode: "queue" }));
          }
          for (let i = 0; i < 200; i++) {
            await new Promise(r => setTimeout(r, 20));
            mob.pollAll();
            if ([...panels.values()].every(panel => panel.element.querySelector(".stream-text")?.textContent.includes("Synthetic specialist ready."))) break;
          }
          const results = [...panels].map(([agent, panel]) => ({
            agent, text: panel.element.querySelector(".stream-text")?.textContent,
            errors: panel.element.querySelectorAll(".stream-error").length,
          }));
          return { version: wasm.runtime_version(), results };
        } finally {
          mob.stopPolling();
          wasm.destroy_runtime();
        }
      }, { repoPath: repo, origin }).catch(error => { throw Error(`${error.message}; successful single-member control requests=${controlRequests}; mob startup HTTP requests=${requests - controlRequests}`); });
      assert.equal(result.version, "0.8.40");
      assert.equal(result.results.length, 4);
      for (const panel of result.results) {
        assert.equal(panel.errors, 0, JSON.stringify(result));
        assert.match(panel.text ?? "", /Synthetic specialist ready/, JSON.stringify(result));
      }
      assert.ok(requests >= 4 && requests <= 40, `bounded synthetic provider requests: ${requests}`);
    });
    assert.equal(externalRequests, 0, "external provider/network traffic is forbidden");
  } finally {
    await browser?.close();
    await server.close();
    await rm(work, { recursive: true, force: true });
  }
});

test("E15 download helper never certifies a partial cache", async () => {
  const root = resolve(".work/cache-regression");
  await mkdir(root, { recursive: true });
  const { createServer: httpServer } = await import("node:http");
  let requests = 0, failWasm = false;
  const server = httpServer((req, res) => {
    requests++;
    if (failWasm && req.url.endsWith(".wasm")) { res.writeHead(500); res.end("synthetic failure"); }
    else res.end(req.url.endsWith(".wasm") ? "synthetic wasm" : "synthetic js");
  });
  await new Promise(r => server.listen(0, "127.0.0.1", r));
  const base = `http://127.0.0.1:${server.address().port}`;
  const invoke = () => exec("bash", ["-c", 'source ../download-webcm.sh; download_webcm "$1" "$2"', "test", base, root], { cwd: process.cwd() });
  try {
    for (const shape of ["js-only", "wasm-only", "zero", "complete"]) {
      await rm(root, { recursive: true, force: true }); await mkdir(root);
      if (shape !== "wasm-only") await writeFile(resolve(root, "webcm.mjs"), shape === "zero" ? "" : "cached js");
      if (shape !== "js-only") await writeFile(resolve(root, "webcm.wasm"), shape === "zero" ? "" : "cached wasm");
      const before = requests;
      await invoke();
      assert.equal(requests - before, shape === "complete" ? 0 : 2);
      assert.ok((await readFile(resolve(root, "webcm.mjs"))).length);
      assert.ok((await readFile(resolve(root, "webcm.wasm"))).length);
    }
    await rm(root, { recursive: true, force: true }); await mkdir(root);
    failWasm = true;
    await assert.rejects(invoke);
    failWasm = false;
    const before = requests;
    await invoke();
    assert.equal(requests - before, 2);
    assert.equal(await readFile(resolve(root, "webcm.wasm"), "utf8"), "synthetic wasm");
  } finally {
    await new Promise(r => server.close(r));
    await rm(root, { recursive: true, force: true });
  }
});
