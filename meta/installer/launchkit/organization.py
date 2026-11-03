"""Host-selected organization context, without runtime or credential authority.

Installed distributions can contribute an entry point in
`meerkat_forge.organizations.v1`. Only the explicitly selected entry point is
loaded. Its callable returns the data described by OrganizationPlugin.
"""
from __future__ import annotations

import json
import os
from importlib.metadata import entry_points
from typing import Annotated, Literal

from pydantic import BaseModel, ConfigDict, Field, StringConstraints, model_validator

ENTRY_POINT_GROUP = "meerkat_forge.organizations.v1"
SELECTION_ENV = "META_FORGE_ORGANIZATION"
Identifier = Annotated[str, StringConstraints(pattern=r"^[a-z][a-z0-9_-]{0,63}$")]
Title = Annotated[str, StringConstraints(strip_whitespace=True, min_length=1, max_length=100)]
Hint = Annotated[str, StringConstraints(strip_whitespace=True, min_length=1, max_length=240)]


class StartingExample(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True, strict=True)

    id: Identifier
    title: Title
    outcome: Hint
    initial_brief: Annotated[str, StringConstraints(strip_whitespace=True, min_length=1, max_length=3000)]
    context: Annotated[str, StringConstraints(max_length=2000)] = ""
    integration_hints: list[Hint] = Field(default_factory=list, max_length=8)


class OrganizationPlugin(BaseModel):
    """Visible context only. All fields may reach the guide and its viewers."""

    model_config = ConfigDict(extra="forbid", frozen=True, strict=True)

    schema_version: Literal[1]
    id: Identifier
    version: Annotated[str, StringConstraints(pattern=r"^[A-Za-z0-9][A-Za-z0-9.+_-]{0,63}$")]
    label: Title
    description: Annotated[str, StringConstraints(max_length=500)] = ""
    guide_context: Annotated[str, StringConstraints(max_length=6000)] = ""
    starting_examples: list[StartingExample] = Field(max_length=8)

    @model_validator(mode="before")
    @classmethod
    def exact_version(cls, value):
        if isinstance(value, dict) and type(value.get("schema_version")) is not int:
            raise ValueError("The organization schema version must be an integer.")
        return value

    @model_validator(mode="after")
    def distinct_examples(self):
        ids = [example.id for example in self.starting_examples]
        if len(ids) != len(set(ids)):
            raise ValueError("Starting example IDs must be unique.")
        return self


def organization_snapshot(value: OrganizationPlugin | dict) -> dict:
    """Detach and validate even an already constructed plugin before saving it."""
    if isinstance(value, OrganizationPlugin):
        value = value.model_dump(mode="json")
    checked = OrganizationPlugin.model_validate(value)
    result = checked.model_dump(mode="json")
    if len(json.dumps(result, ensure_ascii=True).encode("utf-8")) > 65536:
        raise ValueError("Organization context is too large.")
    return result


def load_organization_plugin(selection: str | None = None) -> dict | None:
    """Trusted launcher configuration, never a browser or model-selected import.

    An invalid explicit selection stops startup instead of silently showing the
    stock examples. Package code runs with the host's authority, so installation
    and selection belong to the operator, just like any other Python dependency.
    """
    selected = os.environ.get(SELECTION_ENV) if selection is None else selection
    if selected is None:
        return None
    if not isinstance(selected, str) or not selected or len(selected) > 64:
        raise ValueError("Select an installed Forge organization plugin by entry-point name.")
    matches = list(entry_points(group=ENTRY_POINT_GROUP, name=selected))
    if len(matches) != 1:
        raise ValueError("The selected Forge organization plugin must be installed exactly once.")
    try:
        factory = matches[0].load()
        return organization_snapshot(factory())
    except Exception:
        # Validation/import errors can include arbitrary plugin data. Keep it out
        # of the launcher error, including values accidentally put in the plugin.
        raise ValueError("The selected Forge organization plugin could not supply valid version 1 context.") from None


ORGANIZATION_GUIDANCE = """
The guided_setup.organization object is the host-selected organization's saved
context and starting examples. Use those examples when suggesting starting points;
do not substitute stock household or personal examples. They are optional ideas,
not the user's requirements, confirmed facts, agent roster or permissions.
Honor the user's own idea and any corrections. Ask about outcome, audience,
scale, expertise and collaboration before recommending specific integrations.
Integration hints describe relevance only: check actual availability, account
authentication and grants through their existing owners. A plugin installs or
authorizes nothing merely by being selected.
"""
