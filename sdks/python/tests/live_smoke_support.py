"""Shared helpers for Python SDK live smoke tests."""

from __future__ import annotations

import os
import shutil
import subprocess
from contextlib import asynccontextmanager
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Awaitable, Callable, TypeVar
from uuid import uuid4

from meerkat import MeerkatClient

REPO_ROOT = Path(__file__).resolve().parents[3]
WORKSPACE_CANDIDATES = [
    REPO_ROOT / "target" / "debug" / "rkat-rpc",
    REPO_ROOT / "target-codex" / "debug" / "rkat-rpc",
    REPO_ROOT / "target" / "release" / "rkat-rpc",
]
POLL_INTERVAL_SECS = 0.2
POLL_TIMEOUT_SECS = 45.0

T = TypeVar("T")


def _repo_cargo_target_dir() -> Path | None:
    wrapper = REPO_ROOT / "scripts" / "repo-cargo"
    if not wrapper.exists():
        return None
    try:
        result = subprocess.run(
            [str(wrapper), "--print-env"],
            cwd=REPO_ROOT,
            check=True,
            capture_output=True,
            text=True,
            timeout=10,
        )
    except (OSError, subprocess.SubprocessError):
        return None
    for line in result.stdout.splitlines():
        if line.startswith("CARGO_TARGET_DIR="):
            value = line.split("=", 1)[1].strip()
            return Path(value) if value else None
    return None


def resolve_rkat_rpc_path() -> str | None:
    """Resolve a real rkat-rpc binary for live smoke tests."""
    override = os.environ.get("MEERKAT_BIN_PATH")
    if override:
        if "/" in override or "\\" in override:
            candidate = Path(override).expanduser()
            if candidate.is_file():
                return str(candidate)
            return None
        return shutil.which(override)

    env_target_dir = os.environ.get("CARGO_TARGET_DIR")
    if env_target_dir:
        candidate = Path(env_target_dir) / "debug" / "rkat-rpc"
        if candidate.exists():
            return str(candidate)

    repo_target_dir = _repo_cargo_target_dir()
    if repo_target_dir is not None:
        candidate = repo_target_dir / "debug" / "rkat-rpc"
        if candidate.exists():
            return str(candidate)

    for candidate in WORKSPACE_CANDIDATES:
        if candidate.exists():
            return str(candidate)

    return shutil.which("rkat-rpc")


def has_anthropic_api_key() -> bool:
    return bool(
        os.environ.get("RKAT_ANTHROPIC_API_KEY")
        or os.environ.get("ANTHROPIC_API_KEY")
    )


def has_openai_api_key() -> bool:
    return bool(
        os.environ.get("RKAT_OPENAI_API_KEY")
        or os.environ.get("OPENAI_API_KEY")
    )


def has_gemini_api_key() -> bool:
    return bool(
        os.environ.get("RKAT_GEMINI_API_KEY")
        or os.environ.get("GEMINI_API_KEY")
        or os.environ.get("GOOGLE_API_KEY")
    )


def smoke_model() -> str:
    return os.environ.get("SMOKE_MODEL", "claude-sonnet-4-5")


def openai_model() -> str:
    # Default to the current approved OpenAI smoke model per CLAUDE.md;
    # the previous default `gpt-4.1-mini` is obsolete and was silently
    # degrading instruction-following (showed up in s44 as the reviewer
    # failing to repeat the literal `[TS-SWARM]` marker it was told
    # to remember).
    return os.environ.get("SMOKE_MODEL_OPENAI", "gpt-5.4-mini")


def gemini_model() -> str:
    return os.environ.get("SMOKE_MODEL_GEMINI", "gemini-3.1-pro-preview")


def gemini_image_model() -> str:
    return os.environ.get("SMOKE_IMAGE_MODEL_GEMINI", "gemini-3.1-flash-image-preview")


@asynccontextmanager
async def live_client(**connect_kwargs: Any):
    """Open a real SDK client against a local rkat-rpc subprocess."""
    rpc_path = resolve_rkat_rpc_path()
    if rpc_path is None:
        raise RuntimeError("rkat-rpc binary not found")

    client = MeerkatClient(rpc_path)
    await client.connect(**connect_kwargs)
    try:
        yield client
    finally:
        await client.close()


async def raw_request(client: MeerkatClient, method: str, params: dict[str, Any]) -> dict[str, Any]:
    """Issue a raw JSON-RPC request through the SDK transport."""
    return await client._request(method, params)  # noqa: SLF001


def iso_utc_now() -> str:
    return datetime.now(timezone.utc).isoformat().replace("+00:00", "Z")


def make_prompt_input(
    text: str,
    *,
    durability: str = "durable",
    source_type: str = "operator",
    transcript_eligible: bool = True,
    operator_eligible: bool = True,
    turn_metadata: dict[str, Any] | None = None,
) -> dict[str, Any]:
    if turn_metadata is not None:
        turn_metadata = dict(turn_metadata)
        additional = turn_metadata.get("additional_instructions")
        if isinstance(additional, list):
            turn_metadata["additional_instructions"] = [
                {"kind": "system", "body": item} if isinstance(item, str) else item
                for item in additional
            ]
    payload: dict[str, Any] = {
        "input_type": "prompt",
        "header": {
            "id": str(uuid4()),
            "timestamp": iso_utc_now(),
            "source": {"type": source_type},
            "durability": durability,
            "visibility": {
                "transcript_eligible": transcript_eligible,
                "operator_eligible": operator_eligible,
            },
        },
        "text": text,
    }
    if turn_metadata is not None:
        payload["turn_metadata"] = turn_metadata
    return payload


async def wait_for(
    description: str,
    fetch: Callable[[], Awaitable[T]],
    predicate: Callable[[T], bool],
    *,
    timeout_secs: float = POLL_TIMEOUT_SECS,
    interval_secs: float = POLL_INTERVAL_SECS,
) -> T:
    """Poll until predicate(value) succeeds or the timeout elapses."""
    import asyncio

    deadline = asyncio.get_running_loop().time() + timeout_secs
    last_value: T | None = None

    while True:
        last_value = await fetch()
        if predicate(last_value):
            return last_value
        if asyncio.get_running_loop().time() >= deadline:
            raise AssertionError(f"Timed out waiting for {description}. Last value: {last_value!r}")
        await asyncio.sleep(interval_secs)
