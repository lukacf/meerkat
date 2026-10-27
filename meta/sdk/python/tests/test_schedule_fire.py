"""Tests for host-runnable schedule fires (callback/schedule_fire).

Fix 1 SDK seam: ``MetaBuilder.host_runnables([...])`` rides
``runtime_options.host_runnables`` at init, and incoming
``callback/schedule_fire`` requests dispatch to handlers registered via
``MetaRuntime.on_schedule_fire(name, handler)``.
"""
import pytest

from meerkat_meta.agent_builder import CallbackDispatcher
from meerkat_meta.builder import MetaBuilder
from meerkat_meta.runtime import MetaRuntime

OCCURRENCE = {
    "schedule_id": "sch-1",
    "occurrence_id": "occ-1",
    "due_at": "2026-07-10T07:00:00+00:00",
    "payload": {"depth": 3},
}


class TestScheduleFireDispatch:
    @pytest.mark.asyncio
    async def test_sync_handler_receives_occurrence_and_returns_result(self):
        d = CallbackDispatcher()
        seen = []

        def handler(occurrence):
            seen.append(occurrence)
            return {"digest": "sent"}

        d.register_schedule_fire_handler("digest", handler)
        result = await d.handle_callback(
            "callback/schedule_fire",
            {"runnable": "digest", "occurrence": OCCURRENCE},
        )
        assert result == {"digest": "sent"}
        assert seen == [OCCURRENCE]

    @pytest.mark.asyncio
    async def test_async_handler_is_awaited(self):
        d = CallbackDispatcher()

        async def handler(occurrence):
            return {"echo": occurrence["occurrence_id"]}

        d.register_schedule_fire_handler("digest", handler)
        result = await d.handle_callback(
            "callback/schedule_fire",
            {"runnable": "digest", "occurrence": OCCURRENCE},
        )
        assert result == {"echo": "occ-1"}

    @pytest.mark.asyncio
    async def test_unknown_runnable_raises(self):
        d = CallbackDispatcher()
        with pytest.raises(ValueError, match="no schedule-fire handler"):
            await d.handle_callback(
                "callback/schedule_fire",
                {"runnable": "nobody", "occurrence": OCCURRENCE},
            )

    def test_registration_validates_inputs(self):
        d = CallbackDispatcher()
        with pytest.raises(TypeError, match="non-empty string"):
            d.register_schedule_fire_handler("", lambda o: None)
        with pytest.raises(TypeError, match="handler must be callable"):
            d.register_schedule_fire_handler("digest", "not_a_function")


class TestRuntimeSeam:
    @pytest.mark.asyncio
    async def test_on_schedule_fire_registers_on_the_dispatcher(self):
        runtime = MetaRuntime(MetaBuilder()._config)
        runtime.on_schedule_fire("digest", lambda occurrence: {"ok": True})
        result = await runtime._dispatcher.handle_callback(
            "callback/schedule_fire",
            {"runnable": "digest", "occurrence": OCCURRENCE},
        )
        assert result == {"ok": True}

    def test_host_runnables_ride_init_params(self):
        builder = MetaBuilder().host_runnables(["digest", "backup.rotate"])
        runtime = MetaRuntime(builder._config)
        params = runtime._build_init_params()
        assert params["runtime_options"]["host_runnables"] == [
            "digest",
            "backup.rotate",
        ]

    def test_host_runnables_omitted_when_unset(self):
        runtime = MetaRuntime(MetaBuilder()._config)
        params = runtime._build_init_params()
        assert "host_runnables" not in params["runtime_options"]

    def test_host_runnables_rejects_empty_names(self):
        with pytest.raises(ValueError, match="non-empty strings"):
            MetaBuilder().host_runnables(["digest", "  "])
