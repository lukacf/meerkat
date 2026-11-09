#!/usr/bin/env python3
"""Your generated MobKit application. Run `python app.py --help` for options."""
from __future__ import annotations

import argparse
import asyncio
import json
import os
from pathlib import Path
import signal
import sys

from connectors import ConnectorTools, load_environment

ROOT = Path(__file__).resolve().parent


def load_spec():
    spec = json.loads((ROOT / "deployment.json").read_text())
    if spec.get("schemaVersion") != 1:
        raise ValueError("Unsupported deployment.json schemaVersion")
    agents = spec.get("agents", [])
    identities = [agent["identity"] for agent in agents]
    if not identities or len(set(identities)) != len(identities):
        raise ValueError("The roster requires unique agent identities")
    for edge in spec.get("connections", []):
        if edge[0] not in identities or edge[1] not in identities:
            raise ValueError("A connection references an unknown agent")
    return spec


def doctor(spec):
    import importlib.util
    import platform
    checks = [
        ("Python 3.10 or newer", sys.version_info >= (3, 10)),
        ("MobKit SDK installed", importlib.util.find_spec("meerkat_mobkit") is not None),
        ("Gateway installed", Path(os.environ.get("MOBKIT_RPC_GATEWAY_BIN", str(ROOT / ".bin/rpc_gateway"))).is_file()),
        ("Storage setup tool installed", (ROOT / ".bin/mobkit_gateway").is_file()),
        ("AI provider credential", bool(os.environ.get(spec["provider"]["envKey"]))),
    ]
    for connector in spec["connectors"]:
        for key in connector.get("requiredEnv", []):
            checks.append((f"{connector['name']}: {key}", bool(os.environ.get(key))))
    if any(connector["id"] == "telegram" for connector in spec["connectors"]):
        from telegram_bridge import chat_allowlist
        try:
            chat_allowlist(os.environ.get("TELEGRAM_ALLOWED_CHAT_IDS", ""))
            allowed_valid = True
        except ValueError:
            allowed_valid = False
        checks.append(("Telegram numeric chat allowlist", allowed_valid))
    print(f"{spec['name']} on {platform.system()} {platform.machine()}")
    for label, ready in checks:
        print(f"[{'ready' if ready else 'needs setup'}] {label}")
    print("Credentials remain on this machine. Values are never printed.")
    return all(ready for _, ready in checks)


async def smoke_request(agent, *, timeout=90):
    from meerkat_mobkit.errors import RpcError
    admission = await agent.send(
        "Reply with a short greeting, name your role, and say you are ready. Do not call tools."
    )
    baseline = admission.completion_baseline
    if baseline is None:
        raise RuntimeError("The gateway did not return a completion baseline")
    deadline = asyncio.get_running_loop().time() + timeout
    while True:
        remaining = deadline - asyncio.get_running_loop().time()
        if remaining <= 0:
            raise TimeoutError("The smoke request did not complete in time")
        try:
            return await agent.wait_for_completion(baseline, timeout=remaining)
        except RpcError as error:
            if "observation_lane_saturated" not in str(error):
                raise
            # The request was accepted. Retry only its observation, never delivery.
            await asyncio.sleep(0.5)


async def run(spec, *, smoke=False):
    from meerkat_mobkit import MobKit
    from storage_setup import prepare_storage
    from meerkat_mobkit.identity_first_models import DurableAgentSpec, ManagedPeerEdge

    key = spec["provider"]["envKey"]
    if not os.environ.get(key):
        raise RuntimeError(f"{key} is missing. Run .venv/bin/python setup.py to finish setup.")
    gateway = Path(os.environ.get("MOBKIT_RPC_GATEWAY_BIN", str(ROOT / ".bin/rpc_gateway")))
    if not gateway.is_file():
        raise RuntimeError("Gateway is missing. Run bash install.sh first.")
    port = int(os.environ.get("MOBKIT_PORT", str(spec["port"])))
    if not 1024 <= port <= 65535:
        raise ValueError("MOBKIT_PORT must be between 1024 and 65535")

    class Roster:
        async def roster(self, context):
            return [DurableAgentSpec(
                identity=agent["identity"], profile=agent["id"],
                display_name=agent["name"], addressability="addressable",
                runtime_mode_override="turn_driven",
                labels={"role": agent["role"], "console_group": "Your team"},
            ) for agent in spec["agents"]]

    class Topology:
        async def compute_edges(self, target_identities, context):
            admitted = set(target_identities)
            return [ManagedPeerEdge(a=a, b=b) for a, b in spec["connections"]
                    if a in admitted and b in admitted]

    connector_tools = ConnectorTools(ROOT, spec)

    class SessionBuilder:
        async def build_agent(self, draft):
            # Session construction runs on fresh starts and resumed sessions.
            # Runtime tool callbacks must be reattached on every process boot.
            agent = next((a for a in spec["agents"] if a["id"] == draft.profile_name), None)
            if agent is None:
                raise ValueError("The runtime requested an unknown agent profile")
            for tool in connector_tools.for_agent(agent):
                draft.register_tool(tool["name"], tool["handler"],
                                    description=tool["description"], input_schema=tool["schema"])
            if draft.resume_session_id:
                return
            draft.additional_instructions.append(
                "State which connected tools you actually used. If a connector reports missing "
                "configuration, tell the user what to configure. Do not claim access to data you "
                "have not retrieved. External connectors are read-only in this starter. "
                "You may store useful notes in the project database."
            )

    stop = asyncio.Event()
    loop = asyncio.get_running_loop()
    for sig in (signal.SIGINT, signal.SIGTERM):
        loop.add_signal_handler(sig, stop.set)
    state_dir = ROOT / ".mobkit/state"
    state_dir.mkdir(parents=True, exist_ok=True, mode=0o700)
    os.chmod(state_dir, 0o700)
    # Serialize eager materialization to avoid concurrent first-open schema
    # creation in the pinned runtime. This is a supported gateway setting.
    os.environ["MOBKIT_IDENTITY_RESTORE_CONCURRENCY"] = "1"
    async def build_runtime(*, preparing=False):
        return await (
            MobKit.builder().mob(str(ROOT / "config/mob.toml"))
            .gateway(str(gateway)).persistent_state(str(state_dir))
            .roster(Roster()).topology_provider(Topology()).session_service(SessionBuilder())
            .agent_memory(True).console_config(str(ROOT / "config/console.toml"))
            .http_listen(f"127.0.0.1:{0 if preparing else port}")
            .console_auth_required(False).build()
        )

    await prepare_storage(ROOT, lambda: build_runtime(preparing=True))
    if stop.is_set():
        await connector_tools.close()
        return
    runtime = await build_runtime()
    bridge_task = None
    stop_task = None
    try:
        status = await runtime.identity_bootstrap_status()
        if not status.complete or not status.ready or status.counts.broken:
            states = ", ".join(f"{identity}: {entry.state.value}" for identity, entry in status.identities.items())
            raise RuntimeError(f"Agent startup needs attention ({states}). Inspect the gateway log above.")
        print(f"\n{spec['name']} is running.\nOpen {runtime.rust_http_base_url}/console\n", flush=True)
        if smoke:
            answer = await smoke_request(runtime.agent(spec["agents"][0]["identity"]))
            if not answer:
                raise RuntimeError("The live smoke check returned an empty response")
            print("Live provider round-trip succeeded:")
            print(answer)
        else:
            if any(connector["id"] == "telegram" for connector in spec["connectors"]):
                from telegram_bridge import TelegramBridge
                bridge = TelegramBridge(
                    runtime.agent(spec["agents"][0]["identity"]), ROOT / "data/telegram-state.json",
                    connector_tools.required("TELEGRAM_BOT_TOKEN"),
                    connector_tools.required("TELEGRAM_ALLOWED_CHAT_IDS"),
                )
                bridge_task = asyncio.create_task(bridge.run(stop))
                stop_task = asyncio.create_task(stop.wait())
                done, _ = await asyncio.wait({bridge_task, stop_task}, return_when=asyncio.FIRST_COMPLETED)
                if bridge_task in done:
                    await bridge_task
            else:
                await stop.wait()
    finally:
        tasks = [task for task in (bridge_task, stop_task) if task is not None]
        for task in tasks:
            task.cancel()
        await asyncio.gather(*tasks, return_exceptions=True)
        await runtime.shutdown()
        await connector_tools.close()


def main():
    parser = argparse.ArgumentParser(description="Run your agent team")
    parser.add_argument("--doctor", action="store_true", help="Check installation and credential presence")
    parser.add_argument("--smoke", action="store_true", help="Start the team, make one paid AI request, then exit")
    parser.add_argument("--check", action="store_true", help="Validate configuration without running agents")
    args = parser.parse_args()
    os.chdir(ROOT)
    load_environment(ROOT / ".env")
    spec = load_spec()
    if args.check:
        print(f"Valid deployment: {spec['name']} ({len(spec['agents'])} agents)")
        return
    if args.doctor:
        sys.exit(0 if doctor(spec) else 1)
    try:
        from meerkat_mobkit.errors import MobKitError
    except ModuleNotFoundError:
        raise RuntimeError("MobKit SDK is missing. Run bash install.sh first.") from None
    try:
        from storage_setup import project_lock
        with project_lock(ROOT):
            asyncio.run(run(spec, smoke=args.smoke))
    except MobKitError as error:
        raise RuntimeError(f"MobKit could not complete startup or the request: {error}") from None


if __name__ == "__main__":
    try:
        main()
    except (RuntimeError, ValueError, OSError) as error:
        print(f"Setup needs attention: {error}", file=sys.stderr)
        sys.exit(1)
