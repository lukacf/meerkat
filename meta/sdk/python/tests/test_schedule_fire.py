"""Tests for host-runnable schedule fires (callback/schedule_fire).

Fix 1 SDK seam: ``MetaBuilder.host_runnables([...])`` rides
``runtime_options.host_runnables`` at init, and incoming
``callback/schedule_fire`` requests dispatch to handlers registered via
``MetaRuntime.on_schedule_fire(name, handler)``.

A handler given to ``MetaBuilder.on_schedule_fire(name, handler)`` is in
place before the runtime first connects, so a schedule that is due when the
gateway starts finds it while the init is still settling.
"""
import json
import re
import types

import pytest

from meerkat_meta.agent_builder import CallbackDispatcher
from meerkat_meta.builder import MetaBuilder, MetaBuilderConfig
from meerkat_meta.runtime import MetaRuntime

from .test_init_protocol import _gateway, _records

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


FIRE = {"runnable": "digest", "occurrence": OCCURRENCE}


class TestBuilderSeam:
    def test_on_schedule_fire_is_chainable(self):
        builder = MetaBuilder().host_runnables(["digest"])
        assert builder.on_schedule_fire("digest", lambda occurrence: None) is builder

    @pytest.mark.asyncio
    async def test_a_builder_handler_is_on_the_runtime_before_any_connect(self):
        seen = []

        def handler(occurrence):
            seen.append(occurrence)
            return {"digest": "sent"}

        builder = MetaBuilder().host_runnables(["digest"]).on_schedule_fire("digest", handler)
        runtime = MetaRuntime(builder._config)
        result = await runtime._dispatcher.handle_callback("callback/schedule_fire", FIRE)
        assert result == {"digest": "sent"}
        assert seen == [OCCURRENCE]

    @pytest.mark.asyncio
    async def test_an_async_builder_handler_is_awaited(self):
        async def handler(occurrence):
            return {"echo": occurrence["occurrence_id"]}

        builder = MetaBuilder().host_runnables(["digest"]).on_schedule_fire("digest", handler)
        runtime = MetaRuntime(builder._config)
        result = await runtime._dispatcher.handle_callback("callback/schedule_fire", FIRE)
        assert result == {"echo": "occ-1"}

    def test_on_schedule_fire_checks_its_inputs_at_the_call(self):
        builder = MetaBuilder()
        with pytest.raises(TypeError, match="non-empty string"):
            builder.on_schedule_fire("  ", lambda occurrence: None)
        with pytest.raises(TypeError, match="handler must be callable"):
            builder.on_schedule_fire("digest", "not_a_function")

    @pytest.mark.asyncio
    async def test_a_later_builder_handler_for_one_name_replaces_the_earlier(self):
        builder = (
            MetaBuilder()
            .host_runnables(["digest"])
            .on_schedule_fire("digest", lambda occurrence: "first")
            .on_schedule_fire("digest", lambda occurrence: "second")
        )
        runtime = MetaRuntime(builder._config)
        assert await runtime._dispatcher.handle_callback("callback/schedule_fire", FIRE) == "second"

    @pytest.mark.asyncio
    async def test_a_runtime_registration_still_replaces_and_adds(self):
        builder = (
            MetaBuilder()
            .host_runnables(["digest", "backup.rotate"])
            .on_schedule_fire("digest", lambda occurrence: "from the builder")
        )
        runtime = MetaRuntime(builder._config)
        runtime.on_schedule_fire("digest", lambda occurrence: "from the runtime")
        runtime.on_schedule_fire("backup.rotate", lambda occurrence: "added")
        assert (
            await runtime._dispatcher.handle_callback("callback/schedule_fire", FIRE)
            == "from the runtime"
        )
        assert (
            await runtime._dispatcher.handle_callback(
                "callback/schedule_fire", {"runnable": "backup.rotate", "occurrence": OCCURRENCE}
            )
            == "added"
        )

    @pytest.mark.asyncio
    @pytest.mark.parametrize("handler_first", [True, False])
    async def test_a_handler_for_an_undeclared_name_is_refused_at_build(self, handler_first):
        # build() refuses before it starts anything, whichever of the two
        # calls came first: the gateway fires declared names only, so such a
        # handler could never run.
        builder = MetaBuilder()
        if handler_first:
            builder.on_schedule_fire("digest", lambda occurrence: None)
            builder.host_runnables(["backup.rotate"])
        else:
            builder.host_runnables(["backup.rotate"])
            builder.on_schedule_fire("digest", lambda occurrence: None)
        with pytest.raises(ValueError, match="digest"):
            await builder.build()

    @pytest.mark.parametrize("handler_first", [True, False])
    def test_the_order_of_the_declaration_and_the_handler_does_not_matter(self, handler_first):
        builder = MetaBuilder()
        if handler_first:
            builder.on_schedule_fire("digest", lambda occurrence: None)
            builder.host_runnables(["digest"])
        else:
            builder.host_runnables(["digest"])
            builder.on_schedule_fire("digest", lambda occurrence: None)
        builder._validate()

    def test_a_declared_name_with_no_handler_is_allowed_at_build(self):
        MetaBuilder().host_runnables(["digest"])._validate()

    @pytest.mark.asyncio
    @pytest.mark.parametrize("name", ["digest ", " digest", "Digest"])
    async def test_a_name_that_differs_by_padding_or_case_is_undeclared(self, name):
        # Names are compared exactly, as the gateway keeps them.
        builder = (
            MetaBuilder()
            .host_runnables(["digest"])
            .on_schedule_fire(name, lambda occurrence: None)
        )
        with pytest.raises(ValueError, match=re.escape(repr(name))):
            await builder.build()

    @pytest.mark.asyncio
    async def test_the_refusal_comes_before_anything_is_started(self, tmp_path):
        script = _gateway(tmp_path, "never_started", """
            accepted()
        """)
        builder = (
            MetaBuilder()
            .gateway(str(script))
            .host_runnables(["backup.rotate"])
            .on_schedule_fire("digest", lambda occurrence: None)
        )
        with pytest.raises(ValueError, match="digest"):
            await builder.build()
        assert _records(script) == [], "no gateway was started"

    def test_builder_handlers_send_nothing_in_the_init_request(self):
        plain = MetaRuntime(MetaBuilder().host_runnables(["digest"])._config)
        with_handler = MetaRuntime(
            MetaBuilder()
            .host_runnables(["digest"])
            .on_schedule_fire("digest", lambda occurrence: None)
            ._config
        )
        assert json.dumps(with_handler._build_init_params(), sort_keys=True) == json.dumps(
            plain._build_init_params(), sort_keys=True
        )


class TestRuntimeConstructor:
    @pytest.mark.asyncio
    async def test_the_handlers_are_taken_from_any_mapping(self):
        config = MetaBuilder().host_runnables(["digest"])._config
        config.schedule_fire_handlers = types.MappingProxyType(
            {"digest": lambda occurrence: "from a mapping"}
        )
        runtime = MetaRuntime(config)
        assert (
            await runtime._dispatcher.handle_callback("callback/schedule_fire", FIRE)
            == "from a mapping"
        )

    @pytest.mark.parametrize(
        "config",
        [
            object(),
            types.SimpleNamespace(),
            types.SimpleNamespace(schedule_fire_handlers=None),
            types.SimpleNamespace(schedule_fire_handlers=["digest"]),
        ],
    )
    def test_a_config_that_is_no_builder_config_does_not_break_the_constructor(self, config):
        MetaRuntime(config)


class TestFireDuringTheFirstInit:
    """A stand-in gateway fires a declared runnable after it accepted the
    init and before it settles it: the fire of a schedule that is due when
    the gateway starts. The stand-in records the SDK's answer to it.

    The case that registers on the runtime after ``build()`` documents the
    window that remains for a handler registered there: that fire is
    answered before the runtime exists for the program."""

    FIRES_DURING_INIT = """
        accepted()
        emit({"jsonrpc": "2.0", "id": "fire-1", "method": "callback/schedule_fire",
              "params": {"runnable": "digest", "occurrence": {
                  "schedule_id": "sch-1", "occurrence_id": "occ-1",
                  "due_at": "2026-07-10T07:00:00+00:00"}}})
        log({"fire_answer": json.loads(sys.stdin.readline())})
        settled(outcome="ready", http_base_url="http://127.0.0.1:9",
                stdio_shutdown_handshake=True)
        serve_shutdown()
    """

    @pytest.mark.asyncio
    async def test_a_runtime_made_by_hand_has_its_handler_before_connect(self, tmp_path):
        # A host that fills the config and makes the runtime itself, as the
        # bundled installer's generated host does, registers before
        # connect(): no builder is involved, and the handler is there for a
        # fire during the start.
        script = _gateway(tmp_path, "fires_during_init_by_hand", self.FIRES_DURING_INIT)
        order = []

        def handler(occurrence):
            order.append("the handler ran")
            return {"digest": "sent"}

        runtime = MetaRuntime(
            MetaBuilderConfig(gateway_bin=str(script), host_runnables=["digest"])
        )
        runtime.on_schedule_fire("digest", handler)
        try:
            await runtime.connect()
            order.append("connect() returned")
        finally:
            await runtime.shutdown()
        answer = next(
            entry["fire_answer"] for entry in _records(script) if "fire_answer" in entry
        )
        assert answer["id"] == "fire-1"
        assert answer.get("result") == {"digest": "sent"}, answer
        assert order == ["the handler ran", "connect() returned"]

    @pytest.mark.asyncio
    @pytest.mark.parametrize("registered", ["on the builder", "on the runtime after build"])
    async def test_only_a_builder_handler_is_there_for_it(self, tmp_path, registered):
        script = _gateway(tmp_path, "fires_during_init", self.FIRES_DURING_INIT)
        seen = []
        order = []

        def handler(occurrence):
            order.append("the handler ran")
            seen.append(occurrence["occurrence_id"])
            return {"digest": "sent"}

        builder = MetaBuilder().gateway(str(script)).host_runnables(["digest"])
        if registered == "on the builder":
            builder.on_schedule_fire("digest", handler)
        runtime = await builder.build()
        order.append("build() returned")
        try:
            if registered == "on the runtime after build":
                # The earliest a program can register on the runtime: the
                # statement after build() returned it.
                runtime.on_schedule_fire("digest", handler)
        finally:
            await runtime.shutdown()
        answer = next(
            entry["fire_answer"] for entry in _records(script) if "fire_answer" in entry
        )
        assert answer["id"] == "fire-1"
        if registered == "on the builder":
            assert answer.get("result") == {"digest": "sent"}, answer
            assert seen == ["occ-1"]
            # The handler ran while build() was still connecting: it had no
            # runtime to use.
            assert order == ["the handler ran", "build() returned"]
        else:
            assert answer["error"]["code"] == -32000, answer
            assert (
                "no schedule-fire handler registered for runnable: digest"
                in answer["error"]["message"]
            )
            assert seen == []
            assert order == ["build() returned"]
