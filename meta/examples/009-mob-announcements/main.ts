/**
 * Mob announcements: agents hear about what they subscribed to (Meta 0.9.15 and later).
 *
 * One program, six steps, each printed as it happens:
 *
 * 1. launch with mob announcements enabled;
 * 2. the host subscribes the agent `ops` to the topic `deploys`;
 * 3. the host announces: one announcement on that topic, one on another
 *    topic, the first one again, and the first key with other content
 *    (refused);
 * 4. `ops` is owed one batch with the one announcement it subscribed to;
 * 5. the agent `support` subscribes itself with its own tool, the host and
 *    `ops` each announce an incident, and `support` is delivered both;
 * 6. the host cancels the subscription of `ops`; the next announcement on its
 *    topic is captured for nobody.
 *
 * The one who announces never names a receiver: a topic does that.
 *
 * Run it twice on the same state directory. The second run sends the same
 * operation ids and the same announcement keys, so every call is answered
 * from the record of the first run and nothing is delivered twice.
 *
 * Steps 1 to 4 and 6 need no provider key: without one the batch of step 4
 * is sealed and waits for the agent. With OPENAI_API_KEY the batch is
 * delivered, `ops` reads it, and step 5 runs.
 */
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { AnnouncementConflictError, Meta } from "@rkat/meta-sdk";
import type {
  AnnouncementBatch,
  DurableAgentSpec,
  MetaRuntime,
  MobHandle,
  RosterProvider,
} from "@rkat/meta-sdk";

const HERE = dirname(fileURLToPath(import.meta.url));
const MOB = "announcements";
const OPS = { mob_id: MOB, identity: "ops" };
const SUPPORT = { mob_id: MOB, identity: "support" };

// How announcements are batched for a subscriber. The defaults wait up to a
// minute to collect what a member is owed into one batch (and one agent
// turn); this example waits one second so that it can show a delivery at
// once.
const ANNOUNCEMENTS = {
  enabled: true,
  batch: { max_delay_ms: 1000, min_wake_interval_ms: 1000 },
};

// The agents this program keeps. The gateway asks at every start.
const roster: RosterProvider = {
  async roster(): Promise<DurableAgentSpec[]> {
    return ["ops", "support"].map((name) => ({
      identity: name,
      profile: "listener",
      addressability: "addressable",
      displayName: name[0].toUpperCase() + name.slice(1),
      labels: {},
      context: null,
      additionalInstructions: ["Answer in one short sentence."],
    }));
  },
};

function say(step: string, text: string): void {
  console.log(`[${step}] ${text}`);
}

function keys(batch: AnnouncementBatch): string {
  return batch.items.map((item) => item.sourceKey).join(", ");
}

/**
 * The batches of a member, once they hold `items` announcements. Count
 * announcements, not batches: two announcements that arrive within the batch
 * delay come as one batch. With `settled`, also wait until the turn of each
 * batch has ended: `handling` is null until a batch is admitted, and
 * `pending` while its turn runs.
 */
async function batchesOf(
  handle: MobHandle,
  member: { mob_id: string; identity: string },
  items: number,
  settled: boolean,
  timeoutMs = 90_000,
): Promise<AnnouncementBatch[]> {
  const deadline = Date.now() + timeoutMs;
  for (;;) {
    const batches = await handle.announcementDeliveryList({ member });
    const ready =
      batches.reduce((sum, b) => sum + b.items.length, 0) >= items &&
      (!settled || batches.every((b) => b.handling !== null && b.handling.state !== "pending"));
    if (ready || Date.now() > deadline) return batches;
    await new Promise((resolve) => setTimeout(resolve, 500));
  }
}

async function subscribeOps(handle: MobHandle): Promise<void> {
  // The operation id names this change: sent again, it is answered from the
  // record and nothing changes. The subscription id is yours to choose.
  const answered = await handle.announcementSubscriptionPut("subscribe-ops-1", "ops-deploys", OPS, ["deploys"]);
  say("2", `put answered: ${answered.state}, revision ${answered.revision}`);
  // The answer of a repeated operation is the first answer. What the
  // subscription is now, a read says.
  const now = (await handle.announcementSubscriptionGet("ops-deploys")).subscription;
  if (now === null) throw new Error("ops-deploys is a role template, not a subscription");
  say("2", `ops-deploys now: ${now.state}, revision ${now.revision}, topics ${now.topics.join(", ")}`);
}

async function announce(handle: MobHandle): Promise<void> {
  // `captured` is the number of subscriptions the announcement was recorded
  // for, in the same transaction that records it.
  const first = await handle.announce("deploy-41", ["deploys"], "Version 4.2 is live in production.");
  say("3", `deploy-41 on deploys: captured for ${first.captured}`);
  const other = await handle.announce("lunch-1", ["lunch"], "Lunch is at noon.");
  say("3", `lunch-1 on lunch: captured for ${other.captured}`);
  // The source key is what makes an announcement safe to send twice: the
  // same key with the same content replays the first one.
  const again = await handle.announce("deploy-41", ["deploys"], "Version 4.2 is live in production.");
  say("3", `deploy-41 again, same content: replayed ${again.replayed}, sequence ${again.announcement.sequence}`);
  try {
    await handle.announce("deploy-41", ["deploys"], "Version 4.3 is live in production.");
  } catch (error) {
    if (!(error instanceof AnnouncementConflictError)) throw error;
    say("3", `deploy-41 with other content: refused, ${error.kind}`);
  }
}

async function deliveryToOps(runtime: MetaRuntime, handle: MobHandle, live: boolean): Promise<void> {
  const batches = await batchesOf(handle, OPS, 1, live);
  say("4", `ops is owed ${batches.length} batch: ${batches.length > 0 ? keys(batches[0]) : "none"}`);
  if (batches.length === 0) return;
  const batch = batches[0];
  if (!live) {
    // Sealed: closed and owed. Nothing can be delivered to an agent that
    // cannot run, so the batch waits (and is cancelled with the
    // subscription in step 6).
    say("4", `no provider key, so the agent cannot run: the batch is ${batch.state}`);
    return;
  }
  // `state` is about the delivery, `handling` about the turn it started.
  say("4", `the batch is ${batch.state}, its turn ${batch.handling?.state}`);
  const version = await runtime.sendAndWait(
    "ops",
    "Which version went live, by the announcement you received? Reply with the number only.",
  );
  say("4", `ops knows the version: ${version}`);
}

async function agentsAnnounce(runtime: MetaRuntime, handle: MobHandle): Promise<void> {
  const describe = (subs: readonly { state: string; topics: readonly string[] }[]) =>
    subs.map((s) => `${s.state}, topics ${s.topics.join(", ")}`).join(", ");
  let mine = await handle.announcementSubscriptionList({ member: SUPPORT });
  if (mine.length === 0) {
    // An agent subscribes itself with its own tool, in a turn of its own.
    await runtime.sendAndWait(
      "support",
      "Subscribe yourself to mob announcements on the topic incidents, " +
        "with your mob_subscription tool. Then reply with the single word: subscribed",
      { timeoutMs: 120_000 },
    );
    mine = await handle.announcementSubscriptionList({ member: SUPPORT });
    say("5", `support subscribed itself: ${describe(mine)}`);
    const host = await handle.announce(
      "incident-7",
      ["incidents"],
      "Checkout is failing for card payments in the EU region.",
    );
    say("5", `incident-7 by the host: captured for ${host.captured}`);
    // And an agent announces with its own tool. It names a topic, never a
    // receiver: it does not know that support listens.
    await runtime.sendAndWait(
      "ops",
      "Announce to the mob with your mob_announce tool, on the topic incidents: " +
        "the card processor is back. Then reply with the single word: announced",
      { timeoutMs: 120_000 },
    );
  } else {
    say("5", `support subscribed itself in an earlier run: ${describe(mine)}`);
  }
  const batches = await batchesOf(handle, SUPPORT, 2, true);
  // One batch or two: that depends on whether the second announcement came
  // within the batch delay of the first. Each body names its announcers.
  const bodies = batches.map((b) => b.body).join(" ");
  const announcers = [
    ["the host", "by the host"],
    ["the member ops", 'by member "ops"'],
  ]
    .filter(([, mark]) => bodies.includes(mark))
    .map(([name]) => name);
  const delivered = batches.reduce((sum, b) => sum + b.items.length, 0);
  say("5", `support was delivered ${delivered} announcements, announced by: ${announcers.join(", ")}`);
  const heard = await runtime.sendAndWait(
    "support",
    "Is the card processor back, by the announcements you received? Reply yes or no only.",
  );
  say("5", `support knows the card processor is back: ${heard}`);
}

async function cancelOps(handle: MobHandle): Promise<void> {
  // A change names the revision it expects, so a stale caller is refused
  // instead of overwriting a newer change.
  const changed = await handle.announcementSubscriptionCancel("cancel-ops-1", "ops-deploys", 1);
  say("6", `ops-deploys: ${changed.subscription?.state}, revision ${changed.subscription?.revision}`);
  const after = await handle.announce("deploy-42", ["deploys"], "Version 4.3 is live in production.");
  say("6", `deploy-42 on deploys: captured for ${after.captured}`);
}

async function main(): Promise<number> {
  const gateway = process.env.META_GATEWAY_BIN;
  if (!gateway) {
    console.error("Set META_GATEWAY_BIN to the rpc_gateway binary of Meta 0.9.15 or later.");
    return 2;
  }
  const stateDir = process.env.EXAMPLE_STATE_DIR ?? join(HERE, ".state", "typescript");
  const live = Boolean(process.env.OPENAI_API_KEY);

  // Announcements need a state directory and a roster: the gateway refuses
  // to enable them otherwise.
  const runtime = await Meta.builder()
    .gateway(gateway)
    .mob(join(HERE, "mob.toml"))
    .persistentState(stateDir)
    .rosterProvider(roster)
    .mobAnnouncements(ANNOUNCEMENTS)
    .build();
  try {
    const handle = runtime.mobHandle();
    const enabled = (await handle.capabilities()).mobAnnouncements != null;
    say("1", `runtime started, mob announcements enabled: ${enabled ? "yes" : "no"}`);
    await subscribeOps(handle);
    await announce(handle);
    await deliveryToOps(runtime, handle, live);
    if (live) {
      await agentsAnnounce(runtime, handle);
    } else {
      say("5", "skipped: set OPENAI_API_KEY to let the agents subscribe and announce themselves");
    }
    await cancelOps(handle);
    const ops = await handle.announcementDeliveryList({ member: OPS });
    const subscriptions = await handle.announcementSubscriptionList();
    say("end", `batches for ops: ${ops.length}, ${ops.map((b) => b.state).join(", ")}`);
    say(
      "end",
      "subscriptions: " +
        subscriptions.map((s) => `${s.member.identity} on ${s.topics.join("+")} (${s.state})`).join(", "),
    );
  } finally {
    await runtime.shutdown();
  }
  return 0;
}

process.exitCode = await main();
