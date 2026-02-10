"""Skill listing and invocation helpers for the Meerkat SDK."""

from typing import Optional

from .errors import CapabilityUnavailableError, SkillNotFoundError


class SkillHelper:
    """Helpers for working with Meerkat skills.

    Usage:
        helper = SkillHelper(client)
        skills = await helper.list_skills()
        content = await helper.invoke("/shell-patterns", prompt)
    """

    def __init__(self, client):
        self._client = client

    def _require_skills(self):
        """Raise if skills capability is not available."""
        if not self._client.has_capability("skills"):
            raise CapabilityUnavailableError(
                "CAPABILITY_UNAVAILABLE",
                "Skills capability is not available in this runtime",
            )

    async def list_skills(self) -> list[dict]:
        """List available skills from the runtime."""
        self._require_skills()
        caps = await self._client.get_capabilities()
        # Skills are exposed through the capability system
        return [
            {"id": c.id, "description": c.description}
            for c in caps.capabilities
            if c.id == "skills"
        ]

    async def invoke(
        self,
        skill_reference: str,
        prompt: str,
        model: Optional[str] = None,
    ):
        """Invoke a skill by reference (e.g., '/shell-patterns') with a prompt.

        The skill reference is prepended to the prompt for the agent.
        """
        self._require_skills()
        # Skill invocation is done by including the reference in the prompt
        full_prompt = f"{skill_reference} {prompt}"
        return await self._client.create_session(prompt=full_prompt, model=model)
