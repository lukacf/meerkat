/**
 * A deterministic host runnable fired by the gateway's durable scheduler.
 *
 * From meta/sdk/typescript, after `npm ci`:
 *   META_GATEWAY_BIN=/path/to/rpc_gateway npm exec -- tsx examples/host-runnables.ts
 *
 * From the repository root, build a gateway from the matching workspace:
 *   ./scripts/repo-cargo build --locked -p meerkat-meta --bin rpc_gateway
 *
 * This creates an announcement source with a runner. Meta creates its
 * durable interval schedule with target_kind "host_runnable", runnable
 * "local.collect", and payload {mob_id, source_id}. The first fire is due
 * immediately; subsequent fires are a minute apart. No LLM is called.
 *
 * The example logs the collector callback. A real collector would read its
 * provider and commit a step with announcementSourceAdvance. Its handler's
 * return value is acknowledged, not persisted as a schedule result. Throw
 * or reject to fail the occurrence attempt; use occurrence_id to deduplicate
 * external side effects. Handler registration belongs to this process and
 * must be repeated on the builder on each boot, before build() connects.
 *
 * The temporary on-disk state is removed after shutdown. A lasting host
 * supplies a stable persistentState directory instead.
 */
import assert from "node:assert/strict";
import { mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { setTimeout as delay } from "node:timers/promises";

// In an installed application, import these from "@rkat/meta-sdk".
import { Meta, type MetaRuntime } from "../src/index.js";

const gateway = process.env.META_GATEWAY_BIN;
if (!gateway) throw new Error("Set META_GATEWAY_BIN to a matching rpc_gateway binary");

const state = await mkdtemp(join(tmpdir(), "meta-host-runnables-"));
let runtime: MetaRuntime | undefined;
try {
  const mob = join(state, "mob.toml");
  // This example sends no messages. Before messaging the reader, add
  // [profiles.reader.tools] with comms = true to enable its messaging wiring.
  await writeFile(mob, `
[mob]
id = "host-runnables"
[profiles.reader]
model = "claude-haiku-5-5"
runtime_mode = "turn_driven"
external_addressable = true
`);
  const fires: Record<string, unknown>[] = [];
  runtime = await Meta.builder()
    .gateway(gateway)
    .mob(mob)
    .persistentState(join(state, "runtime"))
    .demoLlm()
    .rosterProvider({
      async roster() {
        return [{
          identity: "reader",
          profile: "reader",
          addressability: "addressable" as const,
          displayName: "Reader",
          labels: {},
          context: null,
          additionalInstructions: [],
        }];
      },
    })
    .mobAnnouncements()
    .hostRunnables(["local.collect"])
    // Install before initial connect, including when reopening stored schedules.
    .onScheduleFire("local.collect", async (occurrence) => {
      assert.equal(typeof occurrence.schedule_id, "string");
      assert.equal(typeof occurrence.occurrence_id, "string");
      assert.equal(typeof occurrence.due_at, "string");
      // Schedule writers choose the payload; the fire carries no writer identity.
      // Accept only this collector's fixed source before doing any work.
      const payload = occurrence.payload;
      if (
        typeof payload !== "object" || payload === null || Array.isArray(payload)
        || Object.keys(payload).length !== 2
        || !("mob_id" in payload) || payload.mob_id !== "host-runnables"
        || !("source_id" in payload) || payload.source_id !== "local-feed"
      ) {
        throw new TypeError("local.collect requires only mob_id=host-runnables and source_id=local-feed");
      }
      fires.push(occurrence);
      console.log("Collected local feed:", JSON.stringify(occurrence));
      // No return value: the SDK sends JSON null to acknowledge completion.
    })
    .build();

  const handle = runtime.mobHandle();
  const declared = await handle.announcementSourcePut(
    "declare-local-feed", "local-feed",
    {
      provider: "local.example",
      principal: { kind: "host_attested", attester: "example", account: "local-feed" },
      scope: { kind: "feed", id: "local-feed" },
    },
    { baseline: "delta", deletions: "explicit" },
    ["local.updates"],
    [{ kind: "role", mob_id: "host-runnables", role: "reader" }],
    {
      connection: { kind: "host", id: "local-feed" },
      runner: { host_runnable: "local.collect", every_seconds: 60 },
    },
  );
  const view = await handle.announcementSourceGet("local-feed");
  assert.equal(view.source?.health.runnerScheduledHere, true, "gateway must run the durable schedule host");
  const deadline = Date.now() + 90_000;
  while (fires.length === 0 && Date.now() < deadline) await delay(100);
  assert.ok(fires.length > 0, "durable schedule did not fire within 90 seconds");
  await handle.announcementSourceEnd("end-local-feed", "local-feed", declared.source.revision);
  console.log("Source ended; its interval schedule is stopped.");
} finally {
  // Keep state after a startup or shutdown failure for diagnosis/recovery.
  if (runtime) {
    await runtime.shutdown();
    await rm(state, { recursive: true, force: true });
  }
}
