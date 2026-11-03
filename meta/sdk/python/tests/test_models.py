"""Tests for data models (SessionBuildOptions, DiscoverySpec, etc.)."""
import pytest

from meerkat_meta.models import (
    DiscoverySpec,
    PreSpawnData,
    SessionBuildOptions,
    SessionQuery,
)


class TestSessionBuildOptions:
    def test_add_tools(self):
        opts = SessionBuildOptions()
        opts.add_tools(["tool_a", "tool_b"])
        assert opts.tools == ["tool_a", "tool_b"]

    def test_add_tools_non_string_raises(self):
        opts = SessionBuildOptions()
        with pytest.raises(TypeError):
            opts.add_tools([123])

    def test_to_dict(self):
        opts = SessionBuildOptions(
            app_context={"k": "v"},
            session_id="s-1",
            labels={"env": "test"},
            profile_name="default",
        )
        opts.add_tools(["tool_x"])
        d = opts.to_dict()
        assert d["app_context"] == {"k": "v"}
        assert d["session_id"] == "s-1"
        assert d["labels"] == {"env": "test"}
        assert d["profile_name"] == "default"
        assert d["tools"] == ["tool_x"]

    def test_register_tool(self):
        opts = SessionBuildOptions()
        handler = lambda args: {"result": "ok"}
        opts.register_tool("search", handler)
        assert opts.tools == ["search"]
        assert opts.tool_handlers == {"search": handler}

    def test_register_tool_non_string_raises(self):
        opts = SessionBuildOptions()
        with pytest.raises(TypeError):
            opts.register_tool(123, lambda args: None)

    def test_register_tool_and_add_tools_coexist(self):
        opts = SessionBuildOptions()
        handler = lambda args: "result"
        opts.add_tools(["declared_only"])
        opts.register_tool("with_handler", handler)
        assert opts.tools == ["declared_only", "with_handler"]
        assert opts.tool_handlers == {"with_handler": handler}


class TestDiscoverySpec:
    def test_to_dict(self):
        spec = DiscoverySpec(
            role="prof-1",
            agent_identity="m-1",
            labels={"role": "worker"},
        )
        d = spec.to_dict()
        assert d["role"] == "prof-1"
        assert d["agent_identity"] == "m-1"
        assert d["labels"] == {"role": "worker"}


class TestPreSpawnData:
    def test_to_dict(self):
        ps = PreSpawnData(resume_map={"m-1": "s-1"}, module_id="mod-1")
        d = ps.to_dict()
        assert d["resume_map"] == {"m-1": "s-1"}
        assert d["module_id"] == "mod-1"


class TestSessionQuery:
    def test_to_dict(self):
        q = SessionQuery(agent_type="chat", limit=50)
        d = q.to_dict()
        assert d["agent_type"] == "chat"
        assert d["limit"] == 50
        assert d["include_deleted"] is False
