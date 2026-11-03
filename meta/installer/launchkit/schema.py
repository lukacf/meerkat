"""The versioned, secret-free deployment specification shared with the studio."""

from __future__ import annotations

import re
from typing import Literal

from pydantic import BaseModel, ConfigDict, Field, field_validator, model_validator

Provider = Literal["anthropic", "openai", "gemini"]
Template = Literal["personal", "operations", "research", "custom"]
DEFAULT_MODELS = {"openai": "gpt-6.1-sol", "anthropic": "claude-sonnet-4-6", "gemini": "gemini-3.1-pro-preview"}


def default_model_values(value):
    """Resolve an omitted model without changing an explicit model choice."""
    if not isinstance(value, dict) or "model" in value:
        return value
    provider = value.get("provider", "openai")
    if not isinstance(provider, str) or provider not in DEFAULT_MODELS:
        return value
    return {**value, "model": DEFAULT_MODELS[provider]}


class StrictModel(BaseModel):
    model_config = ConfigDict(extra="forbid", str_strip_whitespace=True)


def check_model_provider(provider: str, model: str) -> None:
    prefixes = {"openai": ("gpt-", "chatgpt-", "o1", "o3", "o4"), "anthropic": ("claude-",), "gemini": ("gemini-",)}
    if not model.startswith(prefixes[provider]):
        raise ValueError(f"Choose a {provider} model so it uses the provider credential requested during setup.")


class Project(StrictModel):
    name: str = Field(min_length=1, max_length=80)
    slug: str = Field(pattern=r"^[a-z][a-z0-9-]{1,39}$")
    description: str = Field(min_length=10, max_length=6000)


class Agent(StrictModel):
    id: str = Field(pattern=r"^[a-z][a-z0-9_-]{0,39}$")
    name: str = Field(min_length=1, max_length=60)
    role: str = Field(min_length=1, max_length=120)
    instructions: str = Field(min_length=10, max_length=4000)


class Cloud(StrictModel):
    project: str = Field(default="", max_length=30)
    region: str = Field(default="europe-west1", pattern=r"^[a-z]+-[a-z]+[0-9]+$")
    zone: str = Field(default="europe-west1-b", pattern=r"^[a-z]+-[a-z]+[0-9]+-[a-z]$")
    machine_type: Literal["e2-small", "e2-medium", "e2-standard-2"] = "e2-medium"


class Advanced(StrictModel):
    port: int = Field(default=4110, ge=1024, le=65535, strict=True)


class DeploymentSpec(StrictModel):
    schema_version: Literal[1] = 1
    project: Project
    template: Template = "personal"
    agents: list[Agent] = Field(min_length=1, max_length=8)
    connections: list[Literal["workspace", "github", "telegram"]] = Field(default_factory=lambda: ["workspace"], max_length=3)
    provider: Provider = "openai"
    model: str = Field(default=DEFAULT_MODELS["openai"], min_length=1, max_length=100, pattern=r"^[a-zA-Z0-9][a-zA-Z0-9._:/-]*$")
    target: Literal["mac", "linux", "gcp"] = "mac"
    storage: Literal["sqlite"] = "sqlite"
    gcp: Cloud = Field(default_factory=Cloud)
    advanced: Advanced = Field(default_factory=Advanced)

    @model_validator(mode="before")
    @classmethod
    def default_model(cls, value):
        return default_model_values(value)

    @model_validator(mode="after")
    def check_consistency(self) -> DeploymentSpec:
        check_model_provider(self.provider, self.model)
        ids = [agent.id for agent in self.agents]
        if len(ids) != len(set(ids)):
            raise ValueError("Each agent needs a unique ID.")
        if len(self.connections) != len(set(self.connections)):
            raise ValueError("Select each connection only once.")
        if self.target == "gcp":
            if not re.fullmatch(r"[a-z][a-z0-9-]{4,28}[a-z0-9]", self.gcp.project):
                raise ValueError("Enter your Google Cloud project ID (6-30 lowercase letters, numbers, and hyphens).")
            if not self.gcp.zone.startswith(self.gcp.region + "-"):
                raise ValueError("Choose a zone inside your selected region.")
        return self


class DesignRequest(StrictModel):
    description: str = Field(min_length=10, max_length=6000)
    template: Template = "personal"
    provider: Provider = "openai"
    model: str = Field(default=DEFAULT_MODELS["openai"], min_length=1, max_length=100, pattern=r"^[a-zA-Z0-9][a-zA-Z0-9._:/-]*$")

    @model_validator(mode="before")
    @classmethod
    def default_model(cls, value):
        return default_model_values(value)

    @model_validator(mode="after")
    def check_provider(self) -> DesignRequest:
        check_model_provider(self.provider, self.model)
        return self


class DesignResult(StrictModel):
    agents: list[Agent] = Field(min_length=1, max_length=8)
    summary: str = Field(min_length=1, max_length=1000)
    source: Literal["agent", "template"]
    warnings: list[str] = Field(default_factory=list)

    @field_validator("agents")
    @classmethod
    def unique_ids(cls, value: list[Agent]) -> list[Agent]:
        if len({a.id for a in value}) != len(value):
            raise ValueError("The designer returned duplicate agent IDs.")
        return value
