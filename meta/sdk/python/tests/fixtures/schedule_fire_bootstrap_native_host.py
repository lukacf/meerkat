"""One fresh host process for a native schedule/bootstrap regression.

The transport holds delivery of the real init answer to the SDK, while its
reader and callback dispatch keep running. The native gateway may already
be ready: this fixture tests public build completion, not native init order.
"""
from __future__ import annotations

import asyncio
import json
import os
from pathlib import Path
import sqlite3
import sys
import time

import meerkat_meta
from meerkat_meta import Meta
from meerkat_meta._transport import PersistentTransport

RUNNABLE = "bootstrap.collect"
MOB_ID = "schedule-bootstrap"
TERMINAL = {"completed", "delivery_failed", "misfired", "skipped", "superseded"}


def read_occurrence(database: Path, ordinal: int) -> dict | None:
    """Read a consistent schedule/occurrence/receipt snapshot, without writes."""
    with sqlite3.connect(database.as_uri() + "?mode=ro", uri=True) as connection:
        connection.execute("BEGIN")
        schedules = [json.loads(row[0]) for row in connection.execute(
            "SELECT schedule_json FROM schedule_schedules"
        )]
        selected = [row for row in schedules if row["target"].get("runnable") == RUNNABLE]
        if not selected:
            return None
        assert len(selected) == 1, "one schedule belongs to this fixture"
        schedule = selected[0]
        row = connection.execute(
            "SELECT phase, action_at_ms, occurrence_json FROM schedule_occurrences "
            "WHERE schedule_id = ? AND occurrence_ordinal = ?",
            (schedule["schedule_id"], ordinal),
        ).fetchone()
        if row is None:
            return None
        phase, action_at_ms, encoded = row
        occurrence = json.loads(encoded)
        receipts = [json.loads(item[0]) for item in connection.execute(
            "SELECT receipt_json FROM schedule_receipts WHERE occurrence_id = ? "
            "ORDER BY recorded_at_ms, receipt_id", (occurrence["occurrence_id"],),
        )]
        return {"schedule": schedule, "phase": phase, "action_at_ms": action_at_ms,
                "occurrence": occurrence, "receipts": receipts}


async def terminal_occurrence(database: Path, ordinal: int) -> dict:
    async def observe():
        while True:
            stored = await asyncio.to_thread(read_occurrence, database, ordinal)
            if stored and stored["phase"] in TERMINAL and stored["receipts"]:
                return stored
            # Observation cadence only. No elapsed delay releases the init gate.
            await asyncio.sleep(0.025)
    return await asyncio.wait_for(observe(), timeout=30)


class EmptyRoster:
    async def roster(self, context):
        return []


class HeldInitAnswer(PersistentTransport):
    """Keep the genuine native init response pending in public build()."""

    def __init__(self, gateway: str):
        super().__init__(gateway)
        self.init_answer_received = asyncio.Event()
        self.callback_finished = asyncio.Event()
        self.release_init_answer = asyncio.Event()
        self.fires: list[dict] = []

    async def send_async(self, request, *, timeout=None):
        response = await super().send_async(request, timeout=timeout)
        if request.get("method") == "meta/init" and "error" not in response:
            self.init_answer_received.set()
            await self.release_init_answer.wait()
        return response

    def set_callback_handler(self, handler):
        async def record(method, params):
            if method != "callback/schedule_fire":
                return await handler(method, params)
            fire = {"params": params, "observed_ns": time.time_ns(),
                    "init_answer_released": self.release_init_answer.is_set()}
            self.fires.append(fire)
            try:
                result = await handler(method, params)
                fire["result"] = result
                return result
            except Exception as error:
                fire["error"] = str(error)
                raise
            finally:
                self.callback_finished.set()
        super().set_callback_handler(record)


async def run(mode: str, directory: Path):
    database = directory / "state" / "schedule.sqlite"
    gateway = os.environ["META_GATEWAY_BIN"]
    evidence = {"pid": os.getpid(), "sdk_file": meerkat_meta.__file__, "mode": mode,
                "started_ns": time.time_ns(), "native_ready_before_callback_allowed": True}
    callbacks = []

    def collect(occurrence):
        callbacks.append({"observed_ns": time.time_ns(), "occurrence": occurrence})

    builder = (Meta.builder().gateway(gateway).mob(str(directory / "mob.toml"))
               .persistent_state(str(directory / "state")).roster(EmptyRoster())
               .mob_announcements().host_runnables([RUNNABLE]))
    runtime = None
    build = None
    transport = None
    try:
        if mode == "seed":
            runtime = await builder.build()
            runtime.on_schedule_fire(RUNNABLE, collect)
            await runtime.mob_handle().announcement_source_put(
                "declare-bootstrap-feed", "local-feed",
                {"provider": "local.example", "principal": {"kind": "host_attested",
                 "attester": "test", "account": "local-feed"},
                 "scope": {"kind": "feed", "id": "local-feed"}},
                {"baseline": "delta", "deletions": "explicit"}, ["local.updates"],
                [{"kind": "role", "mob_id": MOB_ID, "role": "reader"}],
                connection={"kind": "host", "id": "local-feed"},
                runner={"host_runnable": RUNNABLE, "every_seconds": 60},
            )
            evidence["stored"] = await terminal_occurrence(database, 0)
            assert evidence["stored"]["phase"] == "completed", "seed callback must complete"
            assert len(callbacks) == 1
        else:
            transport = HeldInitAnswer(gateway)
            builder.gateway_transport(transport)
            # Explicit test-only compatibility path: released SDKs offer only
            # runtime registration. They must reach the native failed receipt,
            # not fail early because a future builder method is absent.
            register = getattr(builder, "on_schedule_fire", None)
            if register is None:
                evidence["registration_strategy"] = "released-runtime-after-build"
            else:
                register(RUNNABLE, collect)
                evidence["registration_strategy"] = "builder-before-build"
            build = asyncio.create_task(builder.build())
            await asyncio.wait_for(transport.init_answer_received.wait(), timeout=30)
            await asyncio.wait_for(transport.callback_finished.wait(), timeout=30)
            evidence["stored_while_build_pending"] = await terminal_occurrence(database, 1)
            evidence["receipt_observed_ns"] = time.time_ns()
            evidence["build_pending_at_receipt"] = not build.done()
            evidence["callbacks_while_build_pending"] = list(callbacks)
            evidence["fires"] = transport.fires
            assert evidence["build_pending_at_receipt"], "the init response gate holds build"
            assert not transport.release_init_answer.is_set()
            evidence["init_answer_released_ns"] = time.time_ns()
            transport.release_init_answer.set()
            runtime = await asyncio.wait_for(build, timeout=30)
            evidence["build_returned_ns"] = time.time_ns()
            if register is None:
                runtime.on_schedule_fire(RUNNABLE, collect)
                evidence["runtime_handler_registered_ns"] = time.time_ns()
    finally:
        if transport is not None:
            transport.release_init_answer.set()
        if runtime is None and build is not None:
            if not build.done():
                build.cancel()
            try:
                runtime = await build
            except BaseException:
                pass  # build owns its failed-start cleanup; keep the original error
        try:
            if runtime is not None:
                await runtime.shutdown()
                evidence["shutdown_returned_ns"] = time.time_ns()
                evidence["stored"] = read_occurrence(database, 0 if mode == "seed" else 1)
                if mode == "seed":
                    evidence["next_occurrence"] = read_occurrence(database, 1)
        finally:
            evidence["callbacks"] = callbacks
            evidence["finished_ns"] = time.time_ns()
            (directory / f"{mode}.json").write_text(json.dumps(evidence, indent=2) + "\n")


if __name__ == "__main__":
    mode, directory = sys.argv[1:]
    assert mode in {"seed", "restart"}
    asyncio.run(run(mode, Path(directory).resolve()))
