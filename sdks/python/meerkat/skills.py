"""Skill listing and invocation helpers for the Meerkat SDK."""

from typing import Optional

from .errors import CapabilityUnavailableError, SkillNotFoundError


class SkillHelper:
    """Helpers for working with Meerkat skills.

    Skills are loaded by the agent from filesystem and embedded sources.
    The agent's system prompt contains an inventory of available skills.
    To invoke a skill, include its reference (e.g. ``/shell-patterns``)
    in the user prompt.

    Usage::

        helper = SkillHelper(client)
        helper.require_skills()  # raises if skills not available
        result = await helper.invoke(
            session_id, "/shell-patterns", "How do I run a background job?"
        )
    """

    def __init__(self, client):
        self._client = client

    def is_available(self) -> bool:
        """Check if the skills capability is available in this runtime."""
        return self._client.has_capability("skills")

    def require_skills(self) -> None:
        """Raise if skills capability is not available."""
        if not self.is_available():
            raise CapabilityUnavailableError(
                "CAPABILITY_UNAVAILABLE",
                "Skills capability is not available in this runtime. "
                "Build with --features skills to enable.",
            )

    async def invoke(
        self,
        session_id: str,
        skill_reference: str,
        prompt: str,
    ):
        """Invoke a skill within an existing session.

        The skill reference (e.g. ``/shell-patterns``) is prepended to the
        prompt. The agent's system prompt already contains the skill inventory,
        so it knows how to resolve the reference.

        Args:
            session_id: The session to invoke the skill in.
            skill_reference: Skill reference like ``/shell-patterns``.
            prompt: The user's question or request for the skill.

        Returns:
            WireRunResult from the turn.
        """
        self.require_skills()
        full_prompt = f"{skill_reference} {prompt}"
        return await self._client.start_turn(session_id, full_prompt)

    async def invoke_new_session(
        self,
        skill_reference: str,
        prompt: str,
        model: Optional[str] = None,
    ):
        """Create a new session and invoke a skill in the first turn.

        Args:
            skill_reference: Skill reference like ``/shell-patterns``.
            prompt: The user's question or request for the skill.
            model: Optional model override.

        Returns:
            WireRunResult from the session creation.
        """
        self.require_skills()
        full_prompt = f"{skill_reference} {prompt}"
        return await self._client.create_session(prompt=full_prompt, model=model)
