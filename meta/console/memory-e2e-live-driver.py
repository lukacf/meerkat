#!/usr/bin/env python3
"""Live-loop fixture driver for the memory console e2e (UI-P1.C live lane).

Boots the REAL rpc_gateway through the Python SDK (the production path:
SDK -> stdio callbacks -> identity-first restore_flow) with the full
agent-memory plane enabled: sqlite store, recorder tool, distiller
(interaction-triggered, real haiku profile), steward (real dream loop on a
short cadence), llm_writes=observed. NOTHING is seeded — every record,
dream, and memory.* event the console shows is organically produced by the
real model exercising the real memory tool.

Contract (mirrors the live eval bins):
  exit 0  runtime served and shut down cleanly
  exit 2  usage / environment error
  exit 3  live run requested but no provider auth resolves (loud SKIP)

Protocol with the orchestrator (memory-e2e-live.cjs):
  stdout line  READY {"http_base_url": ..., "identity": ..., "state_dir": ...}
  stdin EOF    -> graceful runtime shutdown
"""
from __future__ import annotations

import asyncio
import json
import os
import sys
import tempfile
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(REPO_ROOT / "sdk" / "python"))

from meerkat_meta.builder import Meta  # noqa: E402
from meerkat_meta.identity_first_models import DurableAgentSpec  # noqa: E402
from meerkat_meta.runtime import MetaRuntime  # noqa: E402

IDENTITY = "identity:curator"

MOB_TOML = """\
[mob]
id = "memory-live-e2e"

[profiles.curator]
model = "claude-haiku-4-5"
system_prompt = '''
You are the deployment curator for a small platform team. You have a
durable `memory` tool. Whenever the operator tells you a fact that matters
beyond this conversation (endpoints, schedules, conventions, corrections),
persist it with the memory tool before answering, and consult your memory
when asked to recall. Keep answers to one or two sentences.
'''
external_addressable = true

[profiles.curator.tools]
comms = true
"""


class LiveRoster:
    async def roster(self, context: dict) -> list[DurableAgentSpec]:
        return [
            DurableAgentSpec(
                identity=IDENTITY,
                profile="curator",
                addressability="addressable",
                labels={"display_name": "Curator", "purpose": "deployment memory curator"},
            )
        ]


def resolve_gateway_bin() -> str:
    override = os.environ.get("META_GATEWAY_BIN", "").strip()
    if override:
        return override
    import subprocess

    result = subprocess.run(
        [str(REPO_ROOT / "scripts" / "repo-cargo"), "--print-env", "CARGO_TARGET_DIR"],
        cwd=REPO_ROOT,
        check=True,
        capture_output=True,
        text=True,
    )
    for line in result.stdout.splitlines():
        if line.startswith("CARGO_TARGET_DIR="):
            target = line.split("=", 1)[1].strip()
            if target:
                return str(Path(target) / "debug" / "rpc_gateway")
    return ""


def auth_env_present() -> bool:
    """Fast pre-flight only. The authoritative resolution happens inside the
    gateway through meerkat's factory seam; a turn failing with an auth error
    still exits 3 (the orchestrator watches for interaction_failed)."""
    for key, value in os.environ.items():
        if key.startswith("ANTHROPIC_") and ("KEY" in key or "TOKEN" in key) and value.strip():
            return True
    return False


async def main() -> int:
    if not auth_env_present():
        print(
            "SKIP no resolvable Anthropic auth in the environment "
            "(set ANTHROPIC_API_KEY or an ANTHROPIC_*_TOKEN); live lane not run",
            flush=True,
        )
        return 3

    gateway_bin = resolve_gateway_bin()
    if not gateway_bin or not os.path.isfile(gateway_bin):
        print(
            f"ERROR gateway binary not found at '{gateway_bin}' — run "
            "./scripts/repo-cargo build -p meerkat-meta --bin rpc_gateway",
            flush=True,
        )
        return 2

    state_dir = os.environ.get("META_MEMORY_LIVE_STATE") or tempfile.mkdtemp(
        prefix="meta-memory-live-"
    )
    Path(state_dir).mkdir(parents=True, exist_ok=True)
    mob_toml = Path(state_dir) / "mob.toml"
    mob_toml.write_text(MOB_TOML)

    # The SDK builder has no console_require_app_auth slot yet; the gateway
    # accepts it in runtime_options. Patch the init-params assembly — the
    # console UI must be reachable without an auth token, exactly like the
    # seeded lane runs.
    original_build_init_params = MetaRuntime._build_init_params

    def patched_build_init_params(self):  # type: ignore[no-untyped-def]
        params = original_build_init_params(self)
        params.setdefault("runtime_options", {})["console_require_app_auth"] = False
        return params

    MetaRuntime._build_init_params = patched_build_init_params

    runtime = await (
        Meta.builder()
        .gateway(gateway_bin)
        .mob(str(mob_toml))
        .persistent_state(state_dir)
        .roster(LiveRoster())
        .agent_memory(
            selection="contextual",
            llm_writes="observed",
            recorder_tool=True,
            distiller={"enabled": True, "min_interactions": 1, "runs_per_hour": 12},
            steward={
                "enabled": True,
                "cadence": "*/30s",
                "min_signals": 1,
                "runs_per_day": 8,
            },
        )
        .build()
    )
    try:
        base = runtime.rust_http_base_url
        print(
            "READY "
            + json.dumps(
                {"http_base_url": base, "identity": IDENTITY, "state_dir": state_dir}
            ),
            flush=True,
        )
        # Hold the runtime until the orchestrator closes our stdin.
        loop = asyncio.get_running_loop()
        await loop.run_in_executor(None, sys.stdin.read)
    finally:
        await runtime.shutdown()
    return 0


if __name__ == "__main__":
    sys.exit(asyncio.run(main()))
