"""Session — the first-class handle for multi-turn agent conversations.

Example::

    async with MeerkatClient() as client:
        session = await client.create_session("Summarise this repo")
        print(session.text)

        # Multi-turn (non-streaming)
        result = await session.turn("Now list the open issues")
        print(result.text)

        # Streaming
        async with session.stream("Explain the CI pipeline") as events:
            async for event in events:
                match event:
                    case TextDelta(delta=chunk):
                        print(chunk, end="", flush=True)
            print()
            print(f"Tokens: {events.result.usage.input_tokens}")

        await session.archive()
"""

from __future__ import annotations

import warnings
from typing import TYPE_CHECKING, Any

from .types import RunResult, SkillKey, SkillRef

if TYPE_CHECKING:
    from .client import MeerkatClient
    from .streaming import CommsEventStream, EventStream


def _normalize_skill_ref(skill_ref: SkillRef) -> SkillKey:
    """Convert a skill reference to a canonical :class:`SkillKey`.

    Accepts either a :class:`SkillKey` (passed through) or a legacy string
    of the form ``"<source_uuid>/<skill_name>"`` (with optional leading ``/``).
    Legacy strings emit a :class:`DeprecationWarning`.
    """
    if isinstance(skill_ref, SkillKey):
        return skill_ref

    value = skill_ref[1:] if skill_ref.startswith("/") else skill_ref
    parts = value.split("/")
    if len(parts) < 2:
        raise ValueError(
            f"Invalid skill reference '{skill_ref}'. "
            "Expected '<source_uuid>/<skill_name>'."
        )

    warnings.warn(
        "Legacy string skill references are deprecated; pass SkillKey instead.",
        DeprecationWarning,
        stacklevel=3,
    )
    return SkillKey(source_uuid=parts[0], skill_name="/".join(parts[1:]))


class Session:
    """A live agent session.

    Constructed by :meth:`~meerkat.MeerkatClient.create_session` — not
    directly instantiated.

    The most recent :class:`~meerkat.RunResult` is always accessible via
    :attr:`last_result` and the convenience shortcuts :attr:`text`,
    :attr:`usage`, etc.
    """

    __slots__ = ("_client", "_id", "_ref", "_last_result")

    def __init__(
        self,
        client: MeerkatClient,
        result: RunResult,
    ) -> None:
        self._client = client
        self._id = result.session_id
        self._ref = result.session_ref
        self._last_result = result

    # -- Identity ----------------------------------------------------------

    @property
    def id(self) -> str:
        """The stable UUID for this session."""
        return self._id

    @property
    def ref(self) -> str | None:
        """Optional human-readable session reference."""
        return self._ref

    # -- Last result shortcuts ---------------------------------------------

    @property
    def last_result(self) -> RunResult:
        """The most recent :class:`RunResult`."""
        return self._last_result

    @property
    def text(self) -> str:
        """The assistant's text from the last turn."""
        return self._last_result.text

    @property
    def usage(self) -> Any:
        """Token usage from the last turn."""
        return self._last_result.usage

    @property
    def turns(self) -> int:
        """Number of LLM turns in the last run."""
        return self._last_result.turns

    @property
    def tool_calls(self) -> int:
        """Number of tool calls in the last run."""
        return self._last_result.tool_calls

    @property
    def structured_output(self) -> Any:
        """Structured output from the last run, if requested."""
        return self._last_result.structured_output

    # -- Multi-turn --------------------------------------------------------

    async def turn(
        self,
        prompt: str,
        *,
        skill_refs: list[SkillRef] | None = None,
        skill_references: list[str] | None = None,
        flow_tool_overlay: dict[str, Any] | None = None,
    ) -> RunResult:
        """Run another turn on this session (non-streaming).

        Updates :attr:`last_result` and returns the new
        :class:`~meerkat.RunResult`.
        """
        result = await self._client._start_turn(  # noqa: SLF001
            self._id,
            prompt,
            skill_refs=skill_refs,
            skill_references=skill_references,
            flow_tool_overlay=flow_tool_overlay,
        )
        self._last_result = result
        return result

    def stream(
        self,
        prompt: str,
        *,
        skill_refs: list[SkillRef] | None = None,
        skill_references: list[str] | None = None,
        flow_tool_overlay: dict[str, Any] | None = None,
    ) -> EventStream:
        """Run another turn on this session with streaming events.

        Returns an :class:`~meerkat.EventStream` async context manager::

            async with session.stream("prompt") as events:
                async for event in events:
                    ...
                result = events.result

        Updates :attr:`last_result` when the stream completes.
        """
        return self._client._start_turn_streaming(  # noqa: SLF001
            self._id,
            prompt,
            skill_refs=skill_refs,
            skill_references=skill_references,
            flow_tool_overlay=flow_tool_overlay,
            _session=self,
        )

    # -- Lifecycle ---------------------------------------------------------

    async def interrupt(self) -> None:
        """Cancel the currently running turn, if any."""
        await self._client._interrupt(self._id)  # noqa: SLF001

    async def archive(self) -> None:
        """Archive (remove) this session from the server."""
        await self._client._archive(self._id)  # noqa: SLF001

    # -- Skills convenience ------------------------------------------------

    async def invoke_skill(
        self,
        skill_ref: SkillRef,
        prompt: str,
    ) -> RunResult:
        """Invoke a skill in this session.

        Accepts a :class:`~meerkat.SkillKey` or a legacy string reference.
        Sends the structured ``skill_refs`` parameter to the runtime.
        """
        self._client.require_capability("skills")
        canonical = _normalize_skill_ref(skill_ref)
        return await self.turn(
            prompt,
            skill_refs=[canonical],
        )

    # -- Comms convenience -------------------------------------------------

    async def send(self, **kwargs: Any) -> dict[str, Any]:
        """Send a comms command scoped to this session."""
        return await self._client._send(self._id, **kwargs)  # noqa: SLF001

    async def peers(self) -> list[dict[str, Any]]:
        """List peers visible to this session's comms runtime."""
        result = await self._client._peers(self._id)  # noqa: SLF001
        return result.get("peers", [])

    async def open_comms_stream(
        self,
        *,
        scope: str = "session",
        interaction_id: str | None = None,
    ) -> "CommsEventStream":
        """Open a comms scoped stream for this session."""
        return await self._client.open_comms_stream(  # noqa: SLF001
            self._id,
            scope=scope,
            interaction_id=interaction_id,
        )

    # -- Dunder ------------------------------------------------------------

    def __repr__(self) -> str:
        ref = f" ref={self._ref!r}" if self._ref else ""
        return f"<Session id={self._id!r}{ref}>"
