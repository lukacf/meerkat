"""Installed-artifact attribution for SDK callback tools.

A host names the artifacts it installed with
``MetaBuilder.installed_artifacts(...)`` (``runtime_options.installed_artifacts``
at init); ``register_tool(..., artifact=...)`` carries a callback tool's
artifact reference on the build wire, where the gateway verifies it before
the build publishes any tool.
"""

import pytest

from meerkat_meta import SessionBuildOptions, ToolArtifactRef
from meerkat_meta.builder import MetaBuilder
from meerkat_meta.identity_first_models import AgentBuildDraft, ExternalToolDef
from meerkat_meta.runtime import MetaRuntime

WEATHER = ToolArtifactRef("ext.weather", "sha256:aa")


def test_register_tool_carries_the_artifact_on_the_build_wire():
    options = SessionBuildOptions()
    options.register_tool("forecast", lambda args: args, artifact=WEATHER)
    assert options.to_dict()["tools"] == [
        {
            "name": "forecast",
            "artifact": {"artifact_id": "ext.weather", "digest": "sha256:aa"},
        }
    ]


def test_a_tool_without_an_artifact_keeps_its_wire_shape():
    options = SessionBuildOptions()
    options.register_tool("plain", lambda args: args)
    assert options.to_dict()["tools"] == ["plain"]


def test_the_customizer_draft_round_trips_the_artifact():
    draft = AgentBuildDraft()
    draft.register_tool("forecast", lambda args: args, artifact=WEATHER)
    wire = draft.external_tools[0].to_dict()
    assert wire["artifact"] == {"artifact_id": "ext.weather", "digest": "sha256:aa"}
    assert ExternalToolDef.from_dict(wire).artifact == WEATHER
    plain = ExternalToolDef("plain", "", {"type": "object"})
    assert "artifact" not in plain.to_dict()


def test_register_tool_refuses_a_non_reference_artifact():
    with pytest.raises(TypeError):
        SessionBuildOptions().register_tool(
            "forecast", lambda args: args, artifact={"artifact_id": "x", "digest": "y"}
        )


@pytest.mark.parametrize(
    ("artifact_id", "digest"),
    [("", "sha256:aa"), ("ext", ""), ("ext weather", "d"), ("ext", "d\n"), ("x" * 257, "d")],
)
def test_malformed_references_are_refused(artifact_id, digest):
    with pytest.raises(ValueError):
        ToolArtifactRef(artifact_id, digest)


def test_installed_artifacts_ride_init_params():
    builder = MetaBuilder().installed_artifacts([WEATHER])
    params = MetaRuntime(builder._config)._build_init_params()
    assert params["runtime_options"]["installed_artifacts"] == [
        {"artifact_id": "ext.weather", "digest": "sha256:aa"}
    ]


def test_installed_artifacts_omitted_when_unset():
    params = MetaRuntime(MetaBuilder()._config)._build_init_params()
    assert "installed_artifacts" not in params.get("runtime_options", {})


def test_one_artifact_with_two_digests_is_refused():
    with pytest.raises(ValueError):
        MetaBuilder().installed_artifacts([WEATHER, ToolArtifactRef("ext.weather", "sha256:bb")])


def test_a_replaced_handler_never_inherits_the_previous_artifact():
    options = SessionBuildOptions()
    options.register_tool("forecast", lambda args: 1, artifact=WEATHER, description="old")
    replacement = lambda args: 2  # noqa: E731
    options.register_tool("forecast", replacement)
    assert options.to_dict()["tools"] == ["forecast"]
    assert options.tool_handlers["forecast"] is replacement

    draft = AgentBuildDraft()
    draft.register_tool("forecast", lambda args: 1, artifact=WEATHER)
    draft.register_tool("forecast", lambda args: 2)
    assert [tool.name for tool in draft.external_tools] == ["forecast"]
    assert draft.external_tools[0].artifact is None


def test_a_refused_replacement_leaves_the_prior_registration_intact():
    original = lambda args: 1  # noqa: E731
    options = SessionBuildOptions()
    options.register_tool("forecast", original, artifact=WEATHER, description="old")
    with pytest.raises(TypeError):
        options.register_tool("forecast", lambda args: 2, artifact={"artifact_id": "x"})
    assert options.tool_handlers["forecast"] is original
    assert options.to_dict()["tools"] == [
        {
            "name": "forecast",
            "description": "old",
            "artifact": {"artifact_id": "ext.weather", "digest": "sha256:aa"},
        }
    ]

    draft = AgentBuildDraft()
    draft.register_tool("forecast", original, artifact=WEATHER)
    with pytest.raises(TypeError):
        draft.register_tool("forecast", lambda args: 2, artifact="ext.weather")
    assert draft.tool_handlers["forecast"] is original
    assert draft.external_tools[0].artifact == WEATHER
