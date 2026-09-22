import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { randomUUID } from "node:crypto";
import { cpSync, existsSync, mkdirSync, readFileSync, readdirSync, rmSync } from "node:fs";
import path from "node:path";
import test from "node:test";
import { setTimeout as delay } from "node:timers/promises";
import { fileURLToPath, pathToFileURL } from "node:url";
import { Mob } from "../../sdks/web/dist/index.js";

const here = path.dirname(fileURLToPath(import.meta.url));
const repo = path.resolve(here, "../..");
const wasmDir = path.join(repo, "sdks/web/wasm");

function run(command, args, env) {
  const result = spawnSync(command, args, {
    cwd: repo, env, encoding: "utf8", timeout: 60_000,
  });
  assert.equal(result.status, 0, `${result.error ?? ""}\n${result.stdout}\n${result.stderr}`);
  return result.stdout;
}

function cliBinary() {
  if (process.env.RKAT_TEST_BIN) return path.resolve(process.env.RKAT_TEST_BIN);
  const output = run(path.join(repo, "scripts/repo-cargo"), ["--print-env"], process.env);
  const target = output.split("\n").find(line => line.startsWith("CARGO_TARGET_DIR="));
  assert.ok(target, "repo-cargo must report its target directory");
  return path.join(target.slice("CARGO_TARGET_DIR=".length), "debug/rkat");
}

test("documented SDK bootstrap accepts normalized spawn results and propagates failures", async () => {
  const readme = readFileSync(path.join(here, "README.md"), "utf8");
  const snippet = readme.match(/```javascript\n([\s\S]*?)\n```/);
  assert.ok(snippet, "The documented SDK bootstrap must remain executable");
  const AsyncFunction = Object.getPrototypeOf(async function () {}).constructor;
  const execute = new AsyncFunction("runtime", "fetch", `${snippet[1]}\nreturn results;`);
  const definition = { id: "documented-team", orchestrator: { profile: "commander" } };
  let calls = 0;
  const success = {
    status: "spawned",
    result: { agent_identity: "commander", member_ref: "session:test-member" },
  };
  const runtime = (entry) => ({
    createMob: async (input) => {
      assert.deepEqual(input, definition);
      return new Mob(definition.id, {
        mob_spawn: async (id, specs) => {
          calls++;
          assert.equal(id, definition.id);
          assert.deepEqual(JSON.parse(specs), [
            { profile: "commander", agent_identity: "commander" },
          ]);
          return JSON.stringify([entry]);
        },
      });
    },
  });
  const fetchDefinition = async (url) => {
    assert.equal(url, "./definition.inline.json");
    return Response.json(definition);
  };
  assert.deepEqual(await execute(runtime(success), fetchDefinition), [{
    mob_id: definition.id,
    agent_identity: "commander",
    member_ref: "session:test-member",
  }]);
  await assert.rejects(execute(runtime({
    status: "failed",
    result: { cause: "wiring_error", message: "synthetic refusal" },
  }), fetchDefinition), /synthetic refusal/);
  await assert.rejects(execute(runtime(success), async () => new Response("", { status: 404 })),
    /Definition fetch failed: 404/);
  assert.equal(calls, 2, "A failed definition fetch must not attempt spawning");
});

test("029/030 emit exact inline skills and spawn every role in actual WASM", {
  timeout: 90_000,
}, async t => {
  const binary = cliBinary();
  assert.ok(existsSync(binary), "Build current rkat first, or set RKAT_TEST_BIN");
  const work = path.join(here, ".work", `regression-${randomUUID()}`);
  mkdirSync(path.join(work, "home"), { recursive: true });
  t.after(() => rmSync(work, { recursive: true, force: true }));
  const env = {
    PATH: process.env.PATH, HOME: path.join(work, "home"), RKAT: binary,
    MEERKAT_WASM: path.join(wasmDir, "meerkat_web_runtime_bg.wasm"),
  };
  const wasm = await import(pathToFileURL(path.join(wasmDir, "meerkat_web_runtime.js")));
  await wasm.default({ module_or_path: readFileSync(env.MEERKAT_WASM) });
  t.diagnostic(`Current prebuilt WASM: ${wasm.runtime_version()}`);

  const requests = [];
  const originalFetch = globalThis.fetch;
  globalThis.fetch = async (input, init) => {
    const request = new Request(input, init);
    assert.equal(request.url, "http://127.0.0.1:1/v1/messages");
    assert.equal(request.method, "POST");
    const body = await request.json();
    requests.push(body);
    assert.ok(requests.length <= 9, "One synthetic turn per role; no autonomous swarm");
    const events = [
      { type: "message_start", message: {
        id: "msg_fixture", type: "message", role: "assistant", model: body.model,
        content: [], stop_reason: null, usage: { input_tokens: 1, output_tokens: 0 },
      } },
      { type: "content_block_start", index: 0, content_block: { type: "text", text: "" } },
      { type: "content_block_delta", index: 0,
        delta: { type: "text_delta", text: "OFFLINE_SKILL_OK" } },
      { type: "content_block_stop", index: 0 },
      { type: "message_delta", delta: { stop_reason: "end_turn" }, usage: { output_tokens: 1 } },
      { type: "message_stop" },
    ];
    return new Response(events.map(event =>
      `event: ${event.type}\ndata: ${JSON.stringify(event)}\n\n`).join(""), {
      status: 200, headers: { "content-type": "text/event-stream" },
    });
  };
  t.after(() => {
    wasm.destroy_runtime();
    globalThis.fetch = originalFetch;
  });

  for (const [exampleName, packName] of [
    ["029-web-incident-war-room-sh", "incident-war-room"],
    ["030-web-dashboard-copilot-sh", "dashboard-copilot"],
  ]) {
    const destination = path.join(work, "checkout with spaces", exampleName);
    mkdirSync(destination, { recursive: true });
    for (const entry of readdirSync(path.join(repo, "examples", exampleName))) {
      if ([".work", "__pycache__"].includes(entry)) continue;
      cpSync(path.join(repo, "examples", exampleName, entry), path.join(destination, entry), {
        recursive: true,
      });
    }
    // Repeated packaging must recreate the host artifact without losing the source.
    for (let iteration = 0; iteration < 2; iteration++) {
      run("bash", [path.join(destination, "examples.sh")], env);
    }
    const sourceRoot = path.join(destination, ".work", packName);
    const bundle = path.join(destination, ".work", `${packName}-web`);
    const source = JSON.parse(readFileSync(path.join(sourceRoot, "definition.json"), "utf8"));
    const definition = JSON.parse(readFileSync(path.join(bundle, "definition.inline.json"), "utf8"));
    assert.deepEqual({ ...definition, skills: source.skills }, source);
    for (const [name, skill] of Object.entries(source.skills)) {
      assert.equal(skill.source, "path", "Source definition must remain portable");
      assert.deepEqual(definition.skills[name], {
        source: "inline",
        content: readFileSync(path.join(sourceRoot, skill.path), "utf8"),
      });
    }

    const bootstrap = () => wasm.init_runtime(readFileSync(path.join(bundle, "mobpack.bin")), JSON.stringify({
      anthropic_api_key: "synthetic-no-network",
      anthropic_base_url: "http://127.0.0.1:1",
      model: "claude-sonnet-4-6",
      mobpack_trust: { policy: "permissive" },
    }));
    bootstrap();
    const firstProfile = Object.keys(source.profiles)[0];
    const pathMob = await wasm.mob_create(JSON.stringify({ ...source, id: `${source.id}-paths` }));
    const rejected = JSON.parse(await wasm.mob_spawn(pathMob, JSON.stringify([
      { profile: firstProfile, agent_identity: firstProfile },
    ])));
    assert.equal(rejected[0].status, "failed");
    assert.match(rejected[0].result.message, /file-based skill path.*not supported on wasm32/);
    assert.equal(JSON.parse(await wasm.mob_lifecycle(pathMob, "destroy")).ok, true);
    wasm.destroy_runtime();

    // Each role gets a fresh runtime: this is a skill-bootstrap contract test,
    // not a test of multi-member wiring or an autonomous incident drill.
    for (const [profile, config] of Object.entries(definition.profiles)) {
      bootstrap();
      const mob = await wasm.mob_create(JSON.stringify(definition));
      try {
        const before = requests.length;
        const result = JSON.parse(await wasm.mob_spawn(mob, JSON.stringify([{
          profile, agent_identity: profile,
          initial_message: "Synthetic offline fixture. Reply OFFLINE_SKILL_OK without tools.",
        }])));
        assert.equal(result[0].status, "spawned", JSON.stringify(result));
        let snapshot;
        const deadline = Date.now() + 5_000;
        do {
          snapshot = JSON.parse(await wasm.mob_member_status(mob, profile));
          assert.equal(snapshot.error, undefined, JSON.stringify(snapshot));
          if (snapshot.output_preview?.includes("OFFLINE_SKILL_OK")) break;
          await delay(10);
        } while (Date.now() < deadline);
        assert.ok(snapshot.output_preview?.includes("OFFLINE_SKILL_OK"), JSON.stringify(snapshot));
        assert.equal(requests.length, before + 1, "Exactly one completed synthetic turn");
        const request = requests[before];
        const system = typeof request.system === "string" ? request.system
          : request.system.map(block => block.text).join("\n");
        for (const skill of config.skills) {
          assert.ok(system.includes(definition.skills[skill].content.trim()),
            `${exampleName}/${profile}: markdown reached the real provider prompt`);
        }
        t.diagnostic(`${exampleName}/${profile}: spawned; exact inline skill in provider request; output observed`);
      } finally {
        assert.equal(JSON.parse(await wasm.mob_lifecycle(mob, "destroy")).ok, true);
        wasm.destroy_runtime();
      }
    }
  }
  assert.equal(requests.length, 9);
  assert.equal(existsSync(path.join(work, "home/.rkat/config.toml")), false);
});
