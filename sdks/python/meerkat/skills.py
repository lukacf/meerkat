"""Skill listing and invocation helpers for the Meerkat SDK."""

from __future__ import annotations

from dataclasses import dataclass
from typing import Union
import warnings

from .errors import CapabilityUnavailableError


@dataclass(frozen=True)
class SkillKey:
    source_uuid: str
    skill_name: str


SkillRef = Union[SkillKey, str]


class SkillHelper:
    """Helpers for working with Meerkat skills."""

    def __init__(self, client):
        self._client = client

    def is_available(self) -> bool:
        return self._client.has_capability("skills")

    def require_skills(self) -> None:
        if not self.is_available():
            raise CapabilityUnavailableError(
                "CAPABILITY_UNAVAILABLE",
                "Skills capability is not available in this runtime. "
                "Build with --features skills to enable.",
            )

    @staticmethod
    def _normalize_ref(skill_ref: SkillRef) -> SkillKey:
        if isinstance(skill_ref, SkillKey):
            return skill_ref

        value = skill_ref[1:] if skill_ref.startswith("/") else skill_ref
        parts = value.split("/")
        if len(parts) < 2:
            raise ValueError(
                f"Invalid legacy skill reference '{skill_ref}'. "
                "Expected '<source_uuid>/<skill_name>'."
            )

        warnings.warn(
            "Legacy string skill references are deprecated; pass SkillKey instead.",
            DeprecationWarning,
            stacklevel=3,
        )
        return SkillKey(source_uuid=parts[0], skill_name="/".join(parts[1:]))

    async def invoke(self, session_id: str, skill_ref: SkillRef, prompt: str):
        self.require_skills()
        canonical = self._normalize_ref(skill_ref)
        return await self._client.start_turn(
            session_id,
            prompt,
            skill_refs=[
                {
                    "source_uuid": canonical.source_uuid,
                    "skill_name": canonical.skill_name,
                }
            ],
        )

    async def invoke_new_session(self, skill_ref: SkillRef, prompt: str, model: str | None = None):
        self.require_skills()
        canonical = self._normalize_ref(skill_ref)
        return await self._client.create_session(
            prompt=prompt,
            model=model,
            skill_refs=[
                {
                    "source_uuid": canonical.source_uuid,
                    "skill_name": canonical.skill_name,
                }
            ],
        )
