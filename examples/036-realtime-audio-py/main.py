#!/usr/bin/env python3
"""036 - Realtime Audio (Python SDK)

Start a realtime OpenAI-backed Meerkat mob member, stream microphone audio into
it, play assistant audio, and print transcript/tool/mob activity as it happens.

Run:
    OPENAI_API_KEY=sk-... python3 main.py
"""

from __future__ import annotations

import argparse
import asyncio
import base64
import contextlib
import os
import signal
import sys
from dataclasses import dataclass, field
from datetime import datetime, timezone
from pathlib import Path
from typing import Any
from uuid import uuid4

from meerkat import MeerkatClient, RealtimeChannel


DEFAULT_REALTIME_MODEL = "gpt-realtime-1.5"
DEFAULT_HELPER_MODEL = "gpt-5.5"
HOST_IDENTITY = "voice-host"


def read_field(value: Any, name: str, default: Any = None) -> Any:
    if isinstance(value, dict):
        return value.get(name, default)
    return getattr(value, name, default)


@dataclass(frozen=True)
class AudioFormat:
    mime_type: str
    sample_rate_hz: int
    channels: int

    @classmethod
    def from_wire(cls, value: Any, *, fallback: "AudioFormat | None" = None) -> "AudioFormat":
        if value is None:
            if fallback is None:
                raise ValueError("missing realtime audio format")
            return fallback
        return cls(
            mime_type=str(read_field(value, "mime_type", fallback.mime_type if fallback else "audio/pcm")),
            sample_rate_hz=int(
                read_field(value, "sample_rate_hz", fallback.sample_rate_hz if fallback else 24_000)
            ),
            channels=int(read_field(value, "channels", fallback.channels if fallback else 1)),
        )

    def input_chunk(self, pcm_bytes: bytes) -> dict[str, Any]:
        return {
            "kind": "audio_chunk",
            "mime_type": self.mime_type,
            "sample_rate_hz": self.sample_rate_hz,
            "channels": self.channels,
            "data": base64.b64encode(pcm_bytes).decode("ascii"),
        }


@dataclass
class SharedState:
    mob: Any | None = None
    notes: list[str] = field(default_factory=list)
    delegation_queue: asyncio.Queue[str] = field(default_factory=asyncio.Queue)
    helper_ids: set[str] = field(default_factory=set)
    printed_helpers: set[str] = field(default_factory=set)
    lock: asyncio.Lock = field(default_factory=asyncio.Lock)


@dataclass
class TextProbeSignals:
    tool_completed: asyncio.Event = field(default_factory=asyncio.Event)
    turn_completed: asyncio.Event = field(default_factory=asyncio.Event)
    channel_closed: asyncio.Event = field(default_factory=asyncio.Event)


class TranscriptPrinter:
    def __init__(self) -> None:
        self._partial_user = ""
        self._assistant_open = False

    def _finish_assistant_line(self) -> None:
        if self._assistant_open:
            print()
            self._assistant_open = False

    def status(self, message: str) -> None:
        self._finish_assistant_line()
        print(f"[status] {message}", flush=True)

    def tool(self, message: str) -> None:
        self._finish_assistant_line()
        print(f"[tool] {message}", flush=True)

    def mob(self, message: str) -> None:
        self._finish_assistant_line()
        print(f"[mob] {message}", flush=True)

    def user_partial(self, text: str) -> None:
        self._finish_assistant_line()
        self._partial_user = text
        print(f"\rYou: {text}", end="", flush=True)

    def user_final(self, text: str) -> None:
        self._finish_assistant_line()
        prefix = "\r" if self._partial_user else ""
        print(f"{prefix}You: {text}")
        self._partial_user = ""

    def assistant_delta(self, delta: str) -> None:
        if not self._assistant_open:
            print("Assistant: ", end="", flush=True)
            self._assistant_open = True
        print(delta, end="", flush=True)

    def turn_completed(self) -> None:
        self._finish_assistant_line()


def build_arg_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Talk to a Meerkat realtime OpenAI mob member from your terminal.",
    )
    parser.add_argument("--model", default=os.environ.get("MEERKAT_REALTIME_MODEL", DEFAULT_REALTIME_MODEL))
    parser.add_argument(
        "--helper-model",
        default=os.environ.get("MEERKAT_REALTIME_HELPER_MODEL", DEFAULT_HELPER_MODEL),
        help="Model used when the voice agent delegates work to a helper mob member.",
    )
    parser.add_argument("--realm", default=None, help="Optional existing Meerkat realm to use.")
    parser.add_argument("--rkat-path", default=os.environ.get("RKAT_RPC", "rkat-rpc"))
    parser.add_argument("--input-device", default=None, help="sounddevice input device name or index.")
    parser.add_argument("--output-device", default=None, help="sounddevice output device name or index.")
    parser.add_argument("--chunk-ms", type=int, default=100, help="Microphone frame size in milliseconds.")
    parser.add_argument("--mic-queue", type=int, default=24, help="Buffered microphone chunks before dropping.")
    parser.add_argument("--no-speaker", action="store_true", help="Do not play assistant audio.")
    parser.add_argument(
        "--text-probe",
        action="store_true",
        help="Send one text realtime turn instead of opening microphone devices.",
    )
    return parser


def voice_host_skill() -> str:
    return """# realtime-voice-host

You are a concise spoken assistant running as a Meerkat realtime mob member.

Rules:
- Keep spoken replies short enough for a live conversation.
- Treat the user's live audio transcript as the source of truth.
- When the user asks you to remember, bookmark, note, or save something, call
  the `voice_session_note` tool with the exact note text.
- When the user asks what notes are saved, call `list_voice_session_notes`.
- When the user asks for a second opinion, deeper reasoning, or explicitly says
  to delegate, call `delegate_to_mob` with a focused prompt for a helper member.
- After a tool returns, summarize the result in one sentence.
"""


def helper_skill() -> str:
    return """# realtime-helper

You are a background helper in a Meerkat mob. Respond with a compact, useful
answer for the voice host to read aloud. Prefer two bullets or less.
"""


def build_mob_definition(*, mob_id: str, realtime_model: str, helper_model: str) -> dict[str, Any]:
    return {
        "id": mob_id,
        "orchestrator": {"profile": "host"},
        "profiles": {
            "host": {
                "model": realtime_model,
                "skills": ["voice-host"],
                "tools": {
                    "builtins": True,
                    "comms": True,
                    "mob": True,
                },
                "peer_description": "Realtime voice host with callback tools and mob delegation.",
                "external_addressable": True,
            },
            "helper": {
                "model": helper_model,
                "skills": ["helper"],
                "tools": {
                    "builtins": True,
                    "comms": True,
                },
                "peer_description": "Background helper for delegated voice requests.",
                "external_addressable": True,
            },
        },
        "skills": {
            "voice-host": {"source": "inline", "content": voice_host_skill()},
            "helper": {"source": "inline", "content": helper_skill()},
        },
        "wiring": {
            "auto_wire_orchestrator": True,
            "role_wiring": [{"a": "host", "b": "helper"}],
        },
    }


async def register_tools(client: MeerkatClient, state: SharedState, printer: TranscriptPrinter) -> None:
    @client.tool(
        "voice_session_note",
        description="Save a short note from the live voice conversation.",
        input_schema={
            "type": "object",
            "properties": {
                "text": {"type": "string", "description": "The note to save."},
            },
            "required": ["text"],
            "additionalProperties": False,
        },
    )
    async def voice_session_note(arguments: dict[str, Any]) -> str:
        text = str(arguments.get("text", "")).strip()
        if not text:
            return "No note text was provided."
        timestamp = datetime.now(timezone.utc).strftime("%H:%M:%SZ")
        async with state.lock:
            state.notes.append(f"{timestamp} {text}")
            count = len(state.notes)
        printer.tool(f"saved note #{count}: {text}")
        return f"Saved note #{count}."

    @client.tool(
        "list_voice_session_notes",
        description="List notes saved during the live voice conversation.",
        input_schema={
            "type": "object",
            "properties": {},
            "additionalProperties": False,
        },
    )
    async def list_voice_session_notes(arguments: dict[str, Any]) -> str:
        del arguments
        async with state.lock:
            notes = list(state.notes)
        if not notes:
            return "No notes have been saved yet."
        return "\n".join(f"{index}. {note}" for index, note in enumerate(notes, start=1))

    @client.tool(
        "delegate_to_mob",
        description="Spawn a helper mob member for a delegated background task.",
        input_schema={
            "type": "object",
            "properties": {
                "prompt": {"type": "string", "description": "Focused task for the helper."},
            },
            "required": ["prompt"],
            "additionalProperties": False,
        },
    )
    async def delegate_to_mob(arguments: dict[str, Any]) -> str:
        prompt = str(arguments.get("prompt", "")).strip()
        if not prompt:
            return "No delegation prompt was provided."
        if state.mob is None:
            return "Mob is not ready yet."
        state.delegation_queue.put_nowait(prompt)
        return "Queued the delegation for a helper mob member."


async def wait_for_host_binding(mob: Any, printer: TranscriptPrinter, timeout_s: float = 45.0) -> None:
    deadline = asyncio.get_running_loop().time() + timeout_s
    last_status = ""
    while True:
        status = await mob.member_status(HOST_IDENTITY)
        realtime_status = str(status.get("realtime_attachment_status", "unknown"))
        current_session_id = status.get("current_session_id")
        label = f"member={status.get('status')} realtime={realtime_status}"
        if label != last_status:
            printer.status(label)
            last_status = label
        if current_session_id and realtime_status in {"binding_ready", "intent_present_unbound", "binding_not_ready"}:
            return
        if asyncio.get_running_loop().time() >= deadline:
            raise TimeoutError(f"timed out waiting for realtime member binding; last status: {status}")
        await asyncio.sleep(0.5)


async def create_voice_mob(
    client: MeerkatClient,
    args: argparse.Namespace,
    printer: TranscriptPrinter,
) -> Any:
    mob_id = f"realtime-audio-{uuid4().hex[:8]}"
    mob = await client.create_mob(
        definition=build_mob_definition(
            mob_id=mob_id,
            realtime_model=args.model,
            helper_model=args.helper_model,
        )
    )
    printer.mob(f"created {mob.id}")
    await mob.spawn(
        profile="host",
        agent_identity=HOST_IDENTITY,
        initial_message=(
            "Initialize as the realtime voice host. Do not answer at length yet; "
            "wait for live microphone input."
        ),
        runtime_mode="turn_driven",
    )
    printer.mob(f"spawned {HOST_IDENTITY}")
    await wait_for_host_binding(mob, printer)
    return mob


async def poll_helpers(state: SharedState, printer: TranscriptPrinter, stop_event: asyncio.Event) -> None:
    while not stop_event.is_set():
        await asyncio.sleep(1.0)
        async with state.lock:
            pending = sorted(state.helper_ids - state.printed_helpers)
            mob = state.mob
        if mob is None:
            continue
        for helper_id in pending:
            with contextlib.suppress(Exception):
                status = await mob.member_status(helper_id)
                if status.get("is_final"):
                    preview = str(status.get("output_preview", "")).strip()
                    printer.mob(f"{helper_id} final: {preview or status.get('status')}")
                    async with state.lock:
                        state.printed_helpers.add(helper_id)


async def delegation_worker(state: SharedState, printer: TranscriptPrinter, stop_event: asyncio.Event) -> None:
    while not stop_event.is_set():
        try:
            prompt = await asyncio.wait_for(state.delegation_queue.get(), timeout=0.25)
        except asyncio.TimeoutError:
            continue
        if state.mob is None:
            printer.mob("dropped delegation because mob is not ready")
            continue

        helper_id = f"helper-{uuid4().hex[:8]}"
        try:
            await state.mob.spawn(
                profile="helper",
                agent_identity=helper_id,
                initial_message=prompt,
                runtime_mode="turn_driven",
            )
            async with state.lock:
                state.helper_ids.add(helper_id)
            printer.mob(f"delegated to {helper_id}: {prompt}")
        except Exception as exc:
            printer.mob(f"delegation failed: {exc}")


async def microphone_sender(
    connection: Any,
    audio_format: AudioFormat,
    args: argparse.Namespace,
    printer: TranscriptPrinter,
    stop_event: asyncio.Event,
) -> None:
    try:
        import sounddevice as sd
    except ModuleNotFoundError as exc:
        raise RuntimeError(
            "Missing audio dependency. Install it with: python3 -m pip install -r requirements.txt"
        ) from exc

    loop = asyncio.get_running_loop()
    queue: asyncio.Queue[bytes] = asyncio.Queue(maxsize=max(1, args.mic_queue))
    dropped = 0
    blocksize = max(1, int(audio_format.sample_rate_hz * args.chunk_ms / 1000))

    def enqueue_chunk(chunk: bytes) -> None:
        nonlocal dropped
        try:
            queue.put_nowait(chunk)
        except asyncio.QueueFull:
            dropped += 1

    def callback(indata: Any, frames: int, time_info: Any, status: Any) -> None:
        del frames, time_info
        if status:
            loop.call_soon_threadsafe(printer.status, f"input device status: {status}")
        chunk = bytes(indata)
        loop.call_soon_threadsafe(enqueue_chunk, chunk)

    printer.status(
        f"microphone open: {audio_format.sample_rate_hz} Hz, {audio_format.channels} channel(s), "
        f"{args.chunk_ms} ms chunks"
    )
    with sd.RawInputStream(
        samplerate=audio_format.sample_rate_hz,
        channels=audio_format.channels,
        dtype="int16",
        blocksize=blocksize,
        device=args.input_device,
        callback=callback,
    ):
        while not stop_event.is_set():
            try:
                chunk = await asyncio.wait_for(queue.get(), timeout=0.25)
            except asyncio.TimeoutError:
                continue
            await connection.send_input(audio_format.input_chunk(chunk))

    if dropped:
        printer.status(f"dropped {dropped} microphone chunks")


async def speaker_player(
    audio_format: AudioFormat | None,
    output_queue: "asyncio.Queue[bytes | None]",
    args: argparse.Namespace,
    printer: TranscriptPrinter,
) -> None:
    if args.no_speaker or args.text_probe:
        while await output_queue.get() is not None:
            pass
        return

    try:
        import sounddevice as sd
    except ModuleNotFoundError as exc:
        raise RuntimeError(
            "Missing audio dependency. Install it with: python3 -m pip install -r requirements.txt"
        ) from exc

    if audio_format is None:
        raise RuntimeError("speaker playback requires realtime audio output capabilities")

    printer.status(f"speaker open: {audio_format.sample_rate_hz} Hz, {audio_format.channels} channel(s)")
    with sd.RawOutputStream(
        samplerate=audio_format.sample_rate_hz,
        channels=audio_format.channels,
        dtype="int16",
        device=args.output_device,
    ) as stream:
        while True:
            chunk = await output_queue.get()
            if chunk is None:
                return
            await asyncio.to_thread(stream.write, chunk)


async def realtime_receiver(
    connection: Any,
    output_queue: "asyncio.Queue[bytes | None]",
    printer: TranscriptPrinter,
    stop_event: asyncio.Event,
    text_probe_signals: TextProbeSignals | None = None,
) -> None:
    try:
        while not stop_event.is_set():
            frame = await connection.recv()
            if frame is None:
                printer.status("realtime websocket closed")
                if text_probe_signals is not None:
                    text_probe_signals.channel_closed.set()
                stop_event.set()
                break
            frame_type = frame.get("type")
            if frame_type == "channel.closed":
                printer.status(f"channel closed: {frame.get('reason', 'done')}")
                if text_probe_signals is not None:
                    text_probe_signals.channel_closed.set()
                stop_event.set()
                break
            if frame_type == "channel.error":
                code = frame.get("code", "unknown")
                message = frame.get("message", "realtime channel error")
                printer.status(f"channel error {code}: {message}")
                stop_event.set()
                raise RuntimeError(f"realtime channel error {code}: {message}")
            if frame_type != "channel.event":
                continue

            event = frame.get("event", {})
            event_type = event.get("type")
            if event_type == "input_transcript_partial":
                printer.user_partial(str(event.get("text", "")))
            elif event_type == "input_transcript_final":
                printer.user_final(str(event.get("text", "")))
            elif event_type == "output_text_delta":
                printer.assistant_delta(str(event.get("delta", "")))
            elif event_type == "output_audio_chunk":
                chunk = event.get("chunk", {})
                data = read_field(chunk, "data", "")
                if data:
                    await output_queue.put(base64.b64decode(data))
            elif event_type == "turn_started":
                printer.status("turn started")
            elif event_type == "turn_committed":
                printer.status("turn committed")
            elif event_type == "turn_completed":
                printer.turn_completed()
                printer.status("turn completed")
                if text_probe_signals is not None:
                    text_probe_signals.turn_completed.set()
            elif event_type == "interrupted":
                printer.status("interrupted")
            elif event_type == "tool_call_requested":
                printer.tool(f"requested {event.get('tool_name')} ({event.get('call_id')})")
            elif event_type == "tool_call_completed":
                printer.tool(f"completed {event.get('call_id')}")
                if text_probe_signals is not None:
                    text_probe_signals.tool_completed.set()
            elif event_type == "tool_call_failed":
                printer.tool(f"failed {event.get('call_id')}: {event.get('error')}")
            elif event_type == "tool_call_timed_out":
                printer.tool(f"timed out {event.get('call_id')} after {event.get('elapsed_ms')} ms")
            elif event_type == "status_changed":
                status = event.get("status", {})
                printer.status(f"channel {read_field(status, 'state', 'unknown')}")
            elif event_type == "needs_reattach":
                printer.status("channel needs reattach")
    finally:
        await output_queue.put(None)


async def wait_for_first_event(
    waits: dict[str, asyncio.Event],
    *,
    timeout_s: float,
) -> str:
    tasks = {
        asyncio.create_task(event.wait(), name=f"wait-{name}"): name
        for name, event in waits.items()
    }
    try:
        done, _pending = await asyncio.wait(
            set(tasks),
            timeout=timeout_s,
            return_when=asyncio.FIRST_COMPLETED,
        )
        if not done:
            raise TimeoutError(f"timed out after {timeout_s:.0f}s")
        for task in done:
            if task.result():
                return tasks[task]
        raise RuntimeError("wait completed without a matching event")
    finally:
        for task in tasks:
            task.cancel()
        await asyncio.gather(*tasks, return_exceptions=True)


async def run_text_probe(
    connection: Any,
    printer: TranscriptPrinter,
    stop_event: asyncio.Event,
    signals: TextProbeSignals,
    timeout_s: float = 45.0,
) -> None:
    await connection.send_input(
        {
            "kind": "text_chunk",
            "text": (
                "This is a text probe for the realtime channel. Say one short sentence, "
                "then call voice_session_note with 'text probe completed'."
            ),
        }
    )
    await connection.commit_turn()
    printer.status("text probe sent; waiting for a tool or turn completion event")
    completed = await wait_for_first_event(
        {
            "tool completion": signals.tool_completed,
            "turn completion": signals.turn_completed,
            "channel close": signals.channel_closed,
        },
        timeout_s=timeout_s,
    )
    if completed == "channel close":
        if signals.tool_completed.is_set():
            completed = "tool completion"
        elif signals.turn_completed.is_set():
            completed = "turn completion"
    if completed == "channel close":
        raise RuntimeError("text probe channel closed before a tool or turn completion event")
    printer.status(f"text probe observed {completed}")
    stop_event.set()


async def wait_for_enter(stop_event: asyncio.Event) -> None:
    loop = asyncio.get_running_loop()
    try:
        stdin_fd = sys.stdin.fileno()
    except (AttributeError, OSError, ValueError):
        await stop_event.wait()
        return

    entered = loop.create_future()
    stop_waiter = asyncio.create_task(stop_event.wait(), name="wait-stop-for-stdin")

    def on_stdin_ready() -> None:
        with contextlib.suppress(Exception):
            sys.stdin.readline()
        if not entered.done():
            entered.set_result(None)

    try:
        loop.add_reader(stdin_fd, on_stdin_ready)
    except (NotImplementedError, OSError, RuntimeError):
        await stop_event.wait()
        return

    try:
        done, _pending = await asyncio.wait(
            {entered, stop_waiter},
            return_when=asyncio.FIRST_COMPLETED,
        )
        if entered in done:
            stop_event.set()
    finally:
        loop.remove_reader(stdin_fd)
        stop_waiter.cancel()
        await asyncio.gather(stop_waiter, return_exceptions=True)


def require_audio_capabilities(open_info: Any) -> tuple[AudioFormat, AudioFormat]:
    capabilities = read_field(open_info, "capabilities", {})
    input_format = read_field(capabilities, "audio_input_format")
    output_format = read_field(capabilities, "audio_output_format")
    if input_format is None:
        raise RuntimeError(
            "Realtime audio input is unavailable. Check OPENAI_API_KEY or your OpenAI "
            "auth binding; without OpenAI realtime credentials rkat-rpc exposes a "
            "text-only fallback channel."
        )
    input_audio = AudioFormat.from_wire(input_format)
    output_audio = AudioFormat.from_wire(output_format, fallback=input_audio)
    return input_audio, output_audio


def require_text_capabilities(open_info: Any) -> None:
    capabilities = read_field(open_info, "capabilities", {})
    input_kinds = set(read_field(capabilities, "input_kinds", []) or [])
    if "text" not in input_kinds:
        raise RuntimeError(
            "Realtime text input is unavailable for this channel. Check the realtime "
            "capabilities exposed by rkat-rpc before using --text-probe."
        )


def build_realtime_channel(
    client: MeerkatClient,
    mob_id: str,
    args: argparse.Namespace,
) -> RealtimeChannel:
    return RealtimeChannel.mob_member(
        client,
        mob_id,
        HOST_IDENTITY,
        turning_mode="explicit_commit" if args.text_probe else "provider_managed",
    )


def check_completed_task(task: asyncio.Task[Any], stop_event: asyncio.Event) -> None:
    if task.cancelled():
        return
    exc = task.exception()
    if exc is not None:
        stop_event.set()
        raise RuntimeError(f"{task.get_name()} failed: {exc}") from exc
    if not stop_event.is_set():
        stop_event.set()
        raise RuntimeError(f"{task.get_name()} stopped unexpectedly")


async def surface_ready_task_failures(
    stop_event: asyncio.Event,
    tasks: list[asyncio.Task[Any]],
) -> None:
    for _ in range(2):
        for task in tasks:
            if task.done():
                check_completed_task(task, stop_event)
        await asyncio.sleep(0)
    for task in tasks:
        if task.done():
            check_completed_task(task, stop_event)


async def wait_for_stop_or_task_failure(
    stop_event: asyncio.Event,
    tasks: list[asyncio.Task[Any]],
) -> None:
    stop_waiter = asyncio.create_task(stop_event.wait(), name="wait-stop")
    pending: set[asyncio.Task[Any]] = {stop_waiter, *tasks}
    try:
        while pending:
            done, pending = await asyncio.wait(pending, return_when=asyncio.FIRST_COMPLETED)
            for task in done:
                if task is stop_waiter:
                    continue
                check_completed_task(task, stop_event)
            if stop_waiter in done:
                await surface_ready_task_failures(stop_event, tasks)
                return
            if stop_event.is_set():
                await surface_ready_task_failures(stop_event, tasks)
                return
    finally:
        stop_waiter.cancel()
        await asyncio.gather(stop_waiter, return_exceptions=True)


async def async_main(argv: list[str] | None = None) -> int:
    args = build_arg_parser().parse_args(argv)
    example_root = Path(__file__).resolve().parent
    printer = TranscriptPrinter()
    state = SharedState()
    stop_event = asyncio.Event()

    loop = asyncio.get_running_loop()
    for sig in (signal.SIGINT, signal.SIGTERM):
        with contextlib.suppress(NotImplementedError):
            loop.add_signal_handler(sig, stop_event.set)

    client = MeerkatClient(rkat_path=args.rkat_path)
    await register_tools(client, state, printer)

    connect_kwargs = {
        "context_root": str(example_root),
        "realm_id": args.realm,
        "isolated": args.realm is None,
    }
    output_queue: asyncio.Queue[bytes | None] = asyncio.Queue()
    connection = None
    tasks: list[asyncio.Task[Any]] = []
    text_probe_signals = TextProbeSignals() if args.text_probe else None

    try:
        await client.connect(**connect_kwargs)
        client.require_capability("mob")
        state.mob = await create_voice_mob(client, args, printer)

        channel = build_realtime_channel(client, state.mob.id, args)
        open_info = await channel.open_info()
        input_audio: AudioFormat | None = None
        output_audio: AudioFormat | None = None
        if args.text_probe:
            require_text_capabilities(open_info)
        else:
            input_audio, output_audio = require_audio_capabilities(open_info)
        connection = await channel.connect_with_open_info(open_info)
        if args.text_probe:
            printer.status("realtime channel ready; running text probe")
        else:
            printer.status("audio channel ready; start talking. Press Enter or Ctrl-C to stop.")

        tasks.append(
            asyncio.create_task(
                realtime_receiver(connection, output_queue, printer, stop_event, text_probe_signals),
                name="realtime receiver",
            )
        )
        tasks.append(
            asyncio.create_task(
                speaker_player(output_audio, output_queue, args, printer),
                name="speaker",
            )
        )
        tasks.append(asyncio.create_task(poll_helpers(state, printer, stop_event), name="helper poller"))
        tasks.append(asyncio.create_task(delegation_worker(state, printer, stop_event), name="delegation worker"))
        if not args.text_probe:
            tasks.append(asyncio.create_task(wait_for_enter(stop_event), name="stdin waiter"))
        if args.text_probe:
            if text_probe_signals is None:
                raise RuntimeError("text probe signals were not initialized")
            tasks.append(
                asyncio.create_task(
                    run_text_probe(connection, printer, stop_event, text_probe_signals),
                    name="text probe",
                )
            )
        else:
            if input_audio is None:
                raise RuntimeError("microphone streaming requires realtime audio input capabilities")
            tasks.append(
                asyncio.create_task(
                    microphone_sender(connection, input_audio, args, printer, stop_event),
                    name="microphone",
                )
            )

        await wait_for_stop_or_task_failure(stop_event, tasks)
    finally:
        stop_event.set()
        if connection is not None:
            with contextlib.suppress(Exception):
                await connection.close()
        await output_queue.put(None)
        for task in tasks:
            task.cancel()
        await asyncio.gather(*tasks, return_exceptions=True)
        if state.notes:
            printer.status("saved notes:")
            for index, note in enumerate(state.notes, start=1):
                print(f"  {index}. {note}")
        await client.close()

    return 0


def main() -> None:
    try:
        raise SystemExit(asyncio.run(async_main()))
    except KeyboardInterrupt:
        raise SystemExit(130)


if __name__ == "__main__":
    main()
