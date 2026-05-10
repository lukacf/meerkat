"""LiveChannel -- session lifecycle wrapper for the ``live/*`` RPC surface.

Replaces the old ``RealtimeChannel`` that was removed in the live-adapter
MVP. Unlike the old class (which managed a WebSocket connection directly),
``LiveChannel`` delegates transport ownership to the caller: :meth:`open`
returns a dict shaped like ``LiveOpenResult`` containing a transport
bootstrap (URL + token) that the caller connects independently.

The class wraps the ``MeerkatClient.live_*`` methods, binding a session id
at construction time so callers do not need to thread ``session_id`` /
``channel_id`` through every call.

Example::

    from meerkat import MeerkatClient, LiveChannel

    async with MeerkatClient() as client:
        session = await client.create_session("Hello")
        channel = LiveChannel.session(client, session.id)

        open_result = await channel.open()
        # Connect to open_result["transport"]["url"] externally

        await channel.commit_input()
        status = await channel.status()
        print(status)

        await channel.close()
"""

from __future__ import annotations

from typing import TYPE_CHECKING, Any, Literal

if TYPE_CHECKING:
    from .client import MeerkatClient
    from .generated.types import LiveRefreshResult


class LiveChannel:
    """Session lifecycle wrapper for the ``live/*`` RPC methods.

    Construct via the :meth:`session` class method, then call :meth:`open`
    to start the live channel. The returned dict carries a transport
    bootstrap (URL + token) for the caller to connect externally.
    """

    def __init__(
        self,
        client: MeerkatClient,
        session_id: str,
        *,
        turning_mode: Literal["provider_managed", "explicit_commit"] | None = None,
    ) -> None:
        self._client = client
        self._session_id = session_id
        self._turning_mode = turning_mode
        self._channel_id: str | None = None

    @classmethod
    def session(
        cls,
        client: MeerkatClient,
        session_id: str,
        *,
        turning_mode: Literal["provider_managed", "explicit_commit"] | None = None,
    ) -> LiveChannel:
        """Create a ``LiveChannel`` bound to a standalone session."""
        return cls(client, session_id, turning_mode=turning_mode)

    @property
    def client(self) -> MeerkatClient:
        """The underlying ``MeerkatClient``."""
        return self._client

    @property
    def session_id(self) -> str:
        """The session id this channel is bound to."""
        return self._session_id

    @property
    def channel_id(self) -> str | None:
        """The channel id assigned by the server after :meth:`open`."""
        return self._channel_id

    # -- Lifecycle ---------------------------------------------------------

    async def open(self) -> dict[str, Any]:
        """Open the live channel. Calls ``live/open``.

        Stores the returned ``channel_id`` for subsequent operations.
        Returns the full ``LiveOpenResult`` dict (``channel_id``,
        ``transport``, ``capabilities``, ``continuity``).
        """
        result = await self._client.live_open(
            self._session_id, turning_mode=self._turning_mode
        )
        self._channel_id = result.get("channel_id") or result.get("channelId")
        return result

    async def close(self) -> dict[str, Any]:
        """Close the live channel. Calls ``live/close``."""
        return await self._client.live_close(self._require_channel_id())

    # -- Status / Refresh --------------------------------------------------

    async def status(self) -> dict[str, Any]:
        """Get the current status of the live channel. Calls ``live/status``."""
        return await self._client.live_status(self._require_channel_id())

    async def refresh(self) -> LiveRefreshResult:
        """Apply mutable session config to the open channel.

        Calls ``live/refresh``. Does NOT replay canonical history. Identity
        changes (model/provider swaps) require close + reopen.
        """
        return await self._client.live_refresh(self._require_channel_id())

    # -- Input -------------------------------------------------------------

    async def send_input_text(self, text: str) -> dict[str, Any]:
        """Send a text input chunk. Calls ``live/send_input``."""
        return await self._client.live_send_input_text(
            self._require_channel_id(), text
        )

    async def send_input_audio(
        self,
        data_base64: str,
        sample_rate_hz: int,
        channels: int,
    ) -> dict[str, Any]:
        """Send a base64-encoded audio chunk. Calls ``live/send_input``."""
        return await self._client.live_send_input_audio(
            self._require_channel_id(), data_base64, sample_rate_hz, channels
        )

    async def send_input_image(
        self, mime: str, data_base64: str
    ) -> dict[str, Any]:
        """Send a base64-encoded image input chunk. Calls ``live/send_input``."""
        return await self._client.live_send_input_image(
            self._require_channel_id(), mime, data_base64
        )

    async def send_input_video_frame(
        self,
        codec: str,
        data_base64: str,
        timestamp_ms: int,
    ) -> dict[str, Any]:
        """Send a base64-encoded video-frame input chunk. Calls ``live/send_input``."""
        return await self._client.live_send_input_video_frame(
            self._require_channel_id(), codec, data_base64, timestamp_ms
        )

    async def commit_input(
        self, response_modality: str | None = None
    ) -> dict[str, Any]:
        """Commit buffered input on the channel. Calls ``live/commit_input``.

        Args:
            response_modality: Optional modality override for this single
                response (``"text"`` or ``"audio"``). ``None`` keeps the
                channel default.
        """
        return await self._client.live_commit_input(
            self._require_channel_id(), response_modality=response_modality
        )

    async def interrupt(self) -> dict[str, Any]:
        """Interrupt the in-progress assistant turn. Calls ``live/interrupt``."""
        return await self._client.live_interrupt(self._require_channel_id())

    async def truncate(
        self,
        item_id: str,
        content_index: int,
        audio_played_ms: int,
    ) -> dict[str, Any]:
        """Truncate assistant output at the client-tracked playback cursor.

        Calls ``live/truncate``.
        """
        return await self._client.live_truncate(
            self._require_channel_id(), item_id, content_index, audio_played_ms
        )

    # -- Internal ----------------------------------------------------------

    def _require_channel_id(self) -> str:
        if self._channel_id is None:
            raise RuntimeError(
                "LiveChannel has not been opened yet -- call open() first"
            )
        return self._channel_id
