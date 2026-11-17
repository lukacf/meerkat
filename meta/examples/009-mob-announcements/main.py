"""Mob announcements: agents hear about what they subscribed to (Meta 0.9.15 and later).

One program, six steps, each printed as it happens:

1. launch with mob announcements enabled;
2. the host subscribes the agent `ops` to the topic `deploys`;
3. the host announces: one announcement on that topic, one on another topic,
   the first one again, and the first key with other content (refused);
4. `ops` is owed one batch with the one announcement it subscribed to;
5. the agent `support` subscribes itself with its own tool, the host and
   `ops` each announce an incident, and `support` is delivered both;
6. the host cancels the subscription of `ops`; the next announcement on its
   topic is captured for nobody.

The one who announces never names a receiver: a topic does that.

Run it twice on the same state directory. The second run sends the same
operation ids and the same announcement keys, so every call is answered from
the record of the first run and nothing is delivered twice.

Steps 1 to 4 and 6 need no provider key: without one the batch of step 4 is
sealed and waits for the agent. With OPENAI_API_KEY the batch is delivered,
`ops` reads it, and step 5 runs.
"""

from __future__ import annotations

import asyncio
import os
import sys
from pathlib import Path

from meerkat_meta import AnnouncementConflictError, DurableAgentSpec, Meta

HERE = Path(__file__).resolve().parent
MOB = "announcements"
OPS = {"mob_id": MOB, "identity": "ops"}
SUPPORT = {"mob_id": MOB, "identity": "support"}

# How announcements are batched for a subscriber. The defaults wait up to a
# minute to collect what a member is owed into one batch (and one agent turn);
# this example waits one second so that it can show a delivery at once.
ANNOUNCEMENTS = {
    "enabled": True,
    "batch": {"max_delay_ms": 1000, "min_wake_interval_ms": 1000},
}


class Roster:
    """The agents this program keeps. The gateway asks at every start."""

    async def roster(self, context):
        return [
            DurableAgentSpec(
                identity=name,
                profile="listener",
                display_name=name.title(),
                additional_instructions=["Answer in one short sentence."],
            )
            for name in ("ops", "support")
        ]


def say(step: str, text: str) -> None:
    print(f"[{step}] {text}", flush=True)


def keys(batch) -> str:
    return ", ".join(item.source_key for item in batch.items)


async def batches_of(handle, member, count: int, settled: bool, timeout: float = 90.0):
    """The batches of a member, once there are `count` of them.

    With `settled`, also wait until the turn of each has ended: `handling`
    is None until a batch is admitted, and `pending` while its turn runs.
    """
    deadline = asyncio.get_running_loop().time() + timeout
    while True:
        batches = await handle.announcement_delivery_list(member=member)
        ready = len(batches) >= count and (
            not settled
            or all(b.handling is not None and b.handling.state != "pending" for b in batches)
        )
        if ready or asyncio.get_running_loop().time() > deadline:
            return batches
        await asyncio.sleep(0.5)


async def subscribe_ops(handle) -> None:
    # The operation id names this change: sent again, it is answered from the
    # record and nothing changes. The subscription id is yours to choose.
    answered = await handle.announcement_subscription_put(
        "subscribe-ops-1", "ops-deploys", OPS, ["deploys"]
    )
    say("2", f"put answered: {answered.state}, revision {answered.revision}")
    # The answer of a repeated operation is the first answer. What the
    # subscription is now, a read says.
    now = (await handle.announcement_subscription_get("ops-deploys")).subscription
    say("2", f"ops-deploys now: {now.state}, revision {now.revision}, topics {', '.join(now.topics)}")


async def announce(handle) -> None:
    # `captured` is the number of subscriptions the announcement was recorded
    # for, in the same transaction that records it.
    first = await handle.announce("deploy-41", ["deploys"], "Version 4.2 is live in production.")
    say("3", f"deploy-41 on deploys: captured for {first.captured}")
    other = await handle.announce("lunch-1", ["lunch"], "Lunch is at noon.")
    say("3", f"lunch-1 on lunch: captured for {other.captured}")
    # The source key is what makes an announcement safe to send twice: the
    # same key with the same content replays the first one.
    again = await handle.announce("deploy-41", ["deploys"], "Version 4.2 is live in production.")
    say("3", f"deploy-41 again, same content: replayed {str(again.replayed).lower()}, sequence {again.announcement.sequence}")
    try:
        await handle.announce("deploy-41", ["deploys"], "Version 4.3 is live in production.")
    except AnnouncementConflictError as error:
        say("3", f"deploy-41 with other content: refused, {error.kind}")


async def delivery_to_ops(runtime, handle, live: bool) -> None:
    batches = await batches_of(handle, OPS, 1, settled=live)
    say("4", f"ops is owed {len(batches)} batch: {keys(batches[0]) if batches else 'none'}")
    if not batches:
        return
    batch = batches[0]
    if not live:
        # Sealed: closed and owed. Nothing can be delivered to an agent that
        # cannot run, so the batch waits (and is cancelled with the
        # subscription in step 6).
        say("4", f"no provider key, so the agent cannot run: the batch is {batch.state}")
        return
    # `state` is about the delivery, `handling` about the turn it started.
    say("4", f"the batch is {batch.state}, its turn {batch.handling.state}")
    version = await runtime.agent("ops").send_and_wait(
        "Which version went live, by the announcement you received? Reply with the number only."
    )
    say("4", f"ops knows the version: {version}")


async def agents_announce(runtime, handle) -> None:
    mine = await handle.announcement_subscription_list(member=SUPPORT)
    if not mine:
        # An agent subscribes itself with its own tool, in a turn of its own.
        await runtime.agent("support").send_and_wait(
            "Subscribe yourself to mob announcements on the topic incidents, "
            "with your mob_subscription tool. Then reply with the single word: subscribed",
            timeout=120,
        )
        mine = await handle.announcement_subscription_list(member=SUPPORT)
        say("5", "support subscribed itself: " + ", ".join(f"{s.state}, topics {', '.join(s.topics)}" for s in mine))
        host = await handle.announce(
            "incident-7", ["incidents"], "Checkout is failing for card payments in the EU region."
        )
        say("5", f"incident-7 by the host: captured for {host.captured}")
        # And an agent announces with its own tool. It names a topic, never
        # a receiver: it does not know that support listens.
        await runtime.agent("ops").send_and_wait(
            "Announce to the mob with your mob_announce tool, on the topic incidents: "
            "the card processor is back. Then reply with the single word: announced",
            timeout=120,
        )
    else:
        say("5", "support subscribed itself in an earlier run: " + ", ".join(f"{s.state}, topics {', '.join(s.topics)}" for s in mine))
    batches = await batches_of(handle, SUPPORT, 2, settled=True)
    announcers = sorted(
        ("the host" if 'by the host' in b.body else "the member ops" if 'by member "ops"' in b.body else "another")
        for b in batches
    )
    say("5", f"support was delivered {len(batches)} batches, announced by: {', '.join(announcers)}")
    heard = await runtime.agent("support").send_and_wait(
        "Is the card processor back, by the announcements you received? Reply yes or no only."
    )
    say("5", f"support knows the card processor is back: {heard}")


async def cancel_ops(handle) -> None:
    # A change names the revision it expects, so a stale caller is refused
    # instead of overwriting a newer change.
    changed = await handle.announcement_subscription_cancel("cancel-ops-1", "ops-deploys", 1)
    say("6", f"ops-deploys: {changed.subscription.state}, revision {changed.subscription.revision}")
    after = await handle.announce("deploy-42", ["deploys"], "Version 4.3 is live in production.")
    say("6", f"deploy-42 on deploys: captured for {after.captured}")


async def main() -> int:
    gateway = os.environ.get("META_GATEWAY_BIN")
    if not gateway:
        print("Set META_GATEWAY_BIN to the rpc_gateway binary of Meta 0.9.15 or later.", file=sys.stderr)
        return 2
    state_dir = Path(os.environ.get("EXAMPLE_STATE_DIR", HERE / ".state" / "python"))
    live = bool(os.environ.get("OPENAI_API_KEY"))

    # Announcements need a state directory and a roster: the gateway refuses
    # to enable them otherwise.
    runtime = await (
        Meta.builder()
        .gateway(gateway)
        .mob(str(HERE / "mob.toml"))
        .persistent_state(str(state_dir))
        .roster(Roster())
        .mob_announcements(ANNOUNCEMENTS)
        .build()
    )
    try:
        handle = runtime.mob_handle()
        enabled = (await handle.capabilities()).mob_announcements is not None
        say("1", f"runtime started, mob announcements enabled: {'yes' if enabled else 'no'}")
        await subscribe_ops(handle)
        await announce(handle)
        await delivery_to_ops(runtime, handle, live)
        if live:
            await agents_announce(runtime, handle)
        else:
            say("5", "skipped: set OPENAI_API_KEY to let the agents subscribe and announce themselves")
        await cancel_ops(handle)
        ops = await handle.announcement_delivery_list(member=OPS)
        subscriptions = await handle.announcement_subscription_list()
        say("end", f"batches for ops: {len(ops)}, {', '.join(b.state for b in ops)}")
        say("end", "subscriptions: " + ", ".join(f"{s.member.identity} on {'+'.join(s.topics)} ({s.state})" for s in subscriptions))
    finally:
        await runtime.shutdown()
    return 0


if __name__ == "__main__":
    sys.exit(asyncio.run(main()))
