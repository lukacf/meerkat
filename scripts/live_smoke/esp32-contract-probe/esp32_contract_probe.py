#!/usr/bin/env python3

from __future__ import annotations

import argparse
import json
import os
import re
import shutil
import signal
import subprocess
import sys
import time
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path
from typing import Sequence


REPO_ROOT = Path(__file__).resolve().parents[3]
REPO_CARGO = REPO_ROOT / "scripts" / "repo-cargo"
PROBE_ROOT = REPO_ROOT / "scripts" / "live_smoke" / "esp32-contract-probe"
FIRMWARE_RUST_ROOT = PROBE_ROOT / "firmware-rust"
DEFAULT_ARTIFACT_ROOT = REPO_ROOT / "artifacts" / "embedded-esp32" / "phase-0"
HOST_CORE_MARKERS = (
    "MKT:HOST_CORE:FACTORY_OK",
    "MKT:HOST_CORE:PROVIDER_OK",
    "MKT:HOST_CORE:RUNTIME_OK",
    "MKT:HOST_CORE:PASS",
)
SINGLE_NODE_MARKERS = (
    "MKT:BOOT:OK",
    "MKT:WIFI:OK",
    "MKT:TIME:OK",
    "MKT:TLS:OK",
    "MKT:PROVIDER:STREAM_START",
    "MKT:PROVIDER:STREAM_DONE",
    "MKT:SINGLE_NODE:PASS",
)
RUST_STACK_MARKERS = (
    "MKT:BOOT:OK",
    "MKT:WIFI:OK",
    "MKT:TIME:OK",
    "MKT:TLS:OK",
    "MKT:PROVIDER:STREAM_START",
    "MKT:PROVIDER:STREAM_DONE",
    "MKT:RUST_STACK:OK",
    "MKT:SINGLE_NODE:PASS",
)
PREP_REQUIRED_TOOLS = ("python3", "cargo", "uvx")
RUST_STACK_TOOLS = ("cargo", "rustc", "rustup")


class MarkerValidationError(RuntimeError):
    """Raised when a marker transcript does not satisfy the expected profile."""


@dataclass(frozen=True)
class CommandSpec:
    label: str
    command: tuple[str, ...]
    cwd: Path


@dataclass(frozen=True)
class CheckSpec:
    name: str
    marker: str
    commands: tuple[CommandSpec, ...]


@dataclass
class RunContext:
    artifact_dir: Path
    run_log: Path
    dry_run: bool

    def emit(self, line: str) -> None:
        print(line, flush=True)
        with self.run_log.open("a", encoding="utf-8") as handle:
            handle.write(f"{line}\n")


@dataclass(frozen=True)
class RustStackPrep:
    port: str
    board: dict[str, object]
    tool_status: dict[str, bool]
    status: str
    error: str | None


def parse_args(argv: Sequence[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Run the internal ESP32 contract probe harness",
    )
    parser.add_argument(
        "--mode",
        required=True,
        choices=(
            "host-core-check",
            "single-node",
            "single-node-rust-stack",
            "negative",
        ),
    )
    parser.add_argument("--host-sim", action="store_true")
    parser.add_argument("--hardware", action="store_true")
    parser.add_argument("--artifact-root", type=Path, default=DEFAULT_ARTIFACT_ROOT)
    parser.add_argument("--dry-run", action="store_true")
    parser.add_argument("--port")
    parser.add_argument("--provider", default=os.environ.get("ESP32_PROBE_PROVIDER", "openai"))
    parser.add_argument(
        "--openai-model",
        default=os.environ.get("ESP32_PROBE_OPENAI_MODEL", "gpt-4.1-mini"),
    )
    parser.add_argument(
        "--monitor-timeout-secs",
        type=int,
        default=int(os.environ.get("ESP32_PROBE_MONITOR_TIMEOUT_SECS", "360")),
    )
    parser.add_argument("--wifi-ssid", default=os.environ.get("ESP32_PROBE_WIFI_SSID"))
    parser.add_argument(
        "--wifi-password",
        default=os.environ.get("ESP32_PROBE_WIFI_PASSWORD"),
    )
    parser.add_argument(
        "--provider-api-key",
        default=os.environ.get("ESP32_PROBE_PROVIDER_API_KEY"),
    )
    parser.add_argument(
        "--allow-missing-provider-key",
        action="store_true",
        help="Permit hardware preflight to continue without a provider key.",
    )
    args = parser.parse_args(argv)

    if args.mode in {"single-node", "single-node-rust-stack"}:
        if args.host_sim == args.hardware:
            parser.error("single-node modes require exactly one of --host-sim or --hardware")
    elif args.host_sim or args.hardware:
        parser.error("--host-sim/--hardware only apply to single-node modes")

    return args


def timestamp_slug() -> str:
    return datetime.now(tz=timezone.utc).strftime("%Y%m%d-%H%M%S")


def ensure_run_context(mode: str, artifact_root: Path, dry_run: bool) -> RunContext:
    artifact_dir = artifact_root / mode / timestamp_slug()
    artifact_dir.mkdir(parents=True, exist_ok=True)
    run_log = artifact_dir / "run.log"
    run_log.write_text("", encoding="utf-8")
    return RunContext(artifact_dir=artifact_dir, run_log=run_log, dry_run=dry_run)


def detect_binary(name: str) -> str | None:
    return shutil.which(name)


def detect_esptool_command() -> list[str]:
    for binary in ("esptool", "esptool.py"):
        if detect_binary(binary):
            return [binary]
    if detect_binary("uvx"):
        return ["uvx", "--from", "esptool", "esptool"]
    raise RuntimeError("could not find esptool; install it or make uvx available")


def choose_port_from_candidates(candidates: Sequence[str]) -> str:
    candidates = sorted(dict.fromkeys(candidates))
    if not candidates:
        raise RuntimeError("could not auto-detect an ESP32 serial port; set ESP32_PROBE_PORT")
    if len(candidates) == 1:
        return candidates[0]

    canonical_groups: dict[str, list[str]] = {}
    for candidate in candidates:
        canonical = candidate.replace("/dev/cu.", "/dev/usb.").replace(
            "/dev/tty.",
            "/dev/usb.",
        )
        canonical_groups.setdefault(canonical, []).append(candidate)

    if len(canonical_groups) == 1:
        grouped_candidates = next(iter(canonical_groups.values()))
        cu_candidates = [
            candidate for candidate in grouped_candidates if candidate.startswith("/dev/cu.")
        ]
        return sorted(cu_candidates or grouped_candidates)[0]

    modem_candidates = [candidate for candidate in candidates if "usbmodem" in candidate]
    if len(modem_candidates) == 1:
        return modem_candidates[0]

    joined = ", ".join(candidates)
    raise RuntimeError(f"multiple serial ports detected; set ESP32_PROBE_PORT explicitly: {joined}")


def detect_port(explicit_port: str | None) -> str:
    if explicit_port:
        return explicit_port

    env_port = os.environ.get("ESP32_PROBE_PORT")
    if env_port:
        return env_port

    patterns = (
        "/dev/cu.usbmodem*",
        "/dev/cu.usbserial*",
        "/dev/tty.usbmodem*",
        "/dev/tty.usbserial*",
    )
    candidates: list[str] = []
    for pattern in patterns:
        candidates.extend(str(path) for path in Path("/").glob(pattern.lstrip("/")))
    return choose_port_from_candidates(candidates)


def sanitize_label(value: str) -> str:
    return re.sub(r"[^A-Za-z0-9_.-]+", "-", value).strip("-").lower() or "command"


def write_json(path: Path, payload: object) -> None:
    path.write_text(json.dumps(payload, indent=2, sort_keys=True) + "\n", encoding="utf-8")


def display_path(path: Path) -> str:
    try:
        return str(path.relative_to(REPO_ROOT))
    except ValueError:
        return str(path)


def extract_markers(text: str) -> list[str]:
    markers: list[str] = []
    for line in text.splitlines():
        index = line.find("MKT:")
        if index < 0:
            continue
        marker = line[index:].strip().split()[0]
        markers.append(marker)
    return markers


def validate_required_markers(markers: Sequence[str], required: Sequence[str]) -> None:
    next_index = 0
    for marker in markers:
        if next_index >= len(required):
            break
        if marker == required[next_index]:
            next_index += 1
    if next_index != len(required):
        missing = required[next_index]
        raise MarkerValidationError(
            f"missing or out-of-order marker: expected {missing} after {list(required[:next_index])}"
        )


def write_marker_report(
    context: RunContext,
    profile_name: str,
    required: Sequence[str],
    transcript_path: Path,
) -> dict[str, object]:
    transcript = transcript_path.read_text(encoding="utf-8")
    markers = extract_markers(transcript)
    validate_required_markers(markers, required)
    report = {
        "profile": profile_name,
        "required": list(required),
        "markers": markers,
        "transcript": display_path(transcript_path),
        "ok": True,
    }
    write_json(context.artifact_dir / "marker_report.json", report)
    return report


def run_command(context: RunContext, spec: CommandSpec, index: int) -> dict[str, object]:
    log_dir = context.artifact_dir / "commands"
    log_dir.mkdir(parents=True, exist_ok=True)
    log_path = log_dir / f"{index:02d}-{sanitize_label(spec.label)}.log"
    context.emit(
        f"[esp32-contract-probe] running {spec.label}: {' '.join(spec.command)}"
    )
    if context.dry_run:
        payload = {
            "label": spec.label,
            "cwd": display_path(spec.cwd),
            "command": list(spec.command),
            "exit_code": 0,
            "dry_run": True,
        }
        write_json(log_path, payload)
        return payload

    completed = subprocess.run(
        list(spec.command),
        cwd=spec.cwd,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
    )
    log_path.write_text(
        "\n".join(
            [
                f"$ {' '.join(spec.command)}",
                "",
                completed.stdout,
                "",
                f"[exit_code={completed.returncode}]",
            ]
        ).rstrip()
        + "\n",
        encoding="utf-8",
    )
    payload = {
        "label": spec.label,
        "cwd": display_path(spec.cwd),
        "command": list(spec.command),
        "exit_code": completed.returncode,
        "log": display_path(log_path),
    }
    if completed.returncode != 0:
        raise RuntimeError(f"{spec.label} failed; see {payload['log']}")
    return payload


def host_core_checks() -> tuple[CheckSpec, ...]:
    return (
        CheckSpec(
            name="factory",
            marker="MKT:HOST_CORE:FACTORY_OK",
            commands=(
                CommandSpec(
                    label="factory_builder_uses_runtime_session_registry_override",
                    command=(
                        str(REPO_CARGO),
                        "test",
                        "-p",
                        "meerkat",
                        "factory_builder_uses_runtime_session_registry_override",
                        "--",
                        "--nocapture",
                    ),
                    cwd=REPO_ROOT,
                ),
                CommandSpec(
                    label="build_agent_with_mock_client_produces_runnable_agent",
                    command=(
                        str(REPO_CARGO),
                        "test",
                        "-p",
                        "meerkat",
                        "--test",
                        "factory_build_agent",
                        "build_agent_with_mock_client_produces_runnable_agent",
                        "--",
                        "--nocapture",
                    ),
                    cwd=REPO_ROOT,
                ),
            ),
        ),
        CheckSpec(
            name="provider",
            marker="MKT:HOST_CORE:PROVIDER_OK",
            commands=(
                CommandSpec(
                    label="test_request_uses_responses_api_endpoint_format",
                    command=(
                        str(REPO_CARGO),
                        "test",
                        "-p",
                        "meerkat-client",
                        "test_request_uses_responses_api_endpoint_format",
                        "--",
                        "--nocapture",
                    ),
                    cwd=REPO_ROOT,
                ),
                CommandSpec(
                    label="test_stream_does_not_duplicate_text_when_completed_replays_output",
                    command=(
                        str(REPO_CARGO),
                        "test",
                        "-p",
                        "meerkat-client",
                        "test_stream_does_not_duplicate_text_when_completed_replays_output",
                        "--",
                        "--nocapture",
                    ),
                    cwd=REPO_ROOT,
                ),
            ),
        ),
        CheckSpec(
            name="runtime",
            marker="MKT:HOST_CORE:RUNTIME_OK",
            commands=(
                CommandSpec(
                    label="control_plane_contract",
                    command=(
                        str(REPO_CARGO),
                        "test",
                        "-p",
                        "meerkat-runtime",
                        "--test",
                        "control_plane_contract",
                        "--",
                        "--ignored",
                        "--nocapture",
                    ),
                    cwd=REPO_ROOT,
                ),
            ),
        ),
    )


def run_host_core_check(context: RunContext) -> int:
    command_results: list[dict[str, object]] = []
    command_index = 1
    try:
        for check in host_core_checks():
            for spec in check.commands:
                result = run_command(context, spec, command_index)
                command_results.append(result)
                command_index += 1
            context.emit(check.marker)
        context.emit("MKT:HOST_CORE:PASS")
        report = write_marker_report(
            context,
            "host-core-check",
            HOST_CORE_MARKERS,
            context.run_log,
        )
        write_json(
            context.artifact_dir / "summary.json",
            {
                "mode": "host-core-check",
                "status": "pass",
                "commands": command_results,
                "marker_report": report,
            },
        )
        return 0
    except Exception as error:
        context.emit(f"MKT:HOST_CORE:FAIL error={json.dumps(str(error))}")
        write_json(
            context.artifact_dir / "summary.json",
            {
                "mode": "host-core-check",
                "status": "fail",
                "commands": command_results,
                "error": str(error),
            },
        )
        return 1


def run_single_node_host_sim(context: RunContext, provider: str) -> int:
    transcript = [
        "MKT:BOOT:OK lane=host-sim",
        'MKT:WIFI:OK ssid="host-sim"',
        "MKT:TIME:OK source=simulated",
        f"MKT:TLS:OK provider={provider}",
        "MKT:PROVIDER:STREAM_START",
        "MKT:PROVIDER:CHUNK seq=1 bytes=5",
        "MKT:PROVIDER:CHUNK seq=2 bytes=7",
        "MKT:PROVIDER:STREAM_DONE",
        "MKT:SINGLE_NODE:PASS lane=host-sim",
    ]
    serial_log = context.artifact_dir / "serial.log"
    serial_log.write_text("\n".join(transcript) + "\n", encoding="utf-8")
    for line in transcript:
        context.emit(line)
    report = write_marker_report(
        context,
        "single-node-host-sim",
        SINGLE_NODE_MARKERS,
        serial_log,
    )
    write_json(
        context.artifact_dir / "summary.json",
        {
            "mode": "single-node",
            "lane": "host-sim",
            "status": "pass",
            "provider": provider,
            "marker_report": report,
        },
    )
    return 0


def ensure_required_tools(tools: Sequence[str]) -> dict[str, bool]:
    return {tool: bool(detect_binary(tool)) for tool in tools}


def resolve_provider_key(provider: str, explicit_value: str | None) -> str | None:
    if explicit_value:
        return explicit_value
    env_candidates: dict[str, tuple[str, ...]] = {
        "openai": ("OPENAI_API_KEY", "RKAT_OPENAI_API_KEY"),
        "anthropic": ("ANTHROPIC_API_KEY", "RKAT_ANTHROPIC_API_KEY"),
        "gemini": ("GEMINI_API_KEY",),
    }
    for key in env_candidates.get(provider, tuple()):
        value = os.environ.get(key)
        if value:
            return value
    return None


def sanitized_subprocess_env(base_env: dict[str, str] | None = None) -> dict[str, str]:
    env = dict(base_env or os.environ)
    for key in (
        "VIRTUAL_ENV",
        "VIRTUAL_ENV_DISABLE_PROMPT",
        "__PYVENV_LAUNCHER__",
        "PYTHONEXECUTABLE",
        "PYTHONHOME",
        "PYTHONPATH",
    ):
        env.pop(key, None)
    return env


def locate_clean_python3() -> Path:
    candidate_paths: list[Path] = []

    explicit = os.environ.get("ESP32_PROBE_PYTHON3")
    if explicit:
        candidate_paths.append(Path(explicit))

    sys_base_prefix = getattr(sys, "base_prefix", None)
    if sys_base_prefix:
        candidate_paths.append(Path(sys_base_prefix) / "bin" / "python3")
        candidate_paths.append(Path(sys_base_prefix) / "bin" / "python3.11")

    candidate_paths.extend(
        [
            Path("/opt/homebrew/opt/python@3.11/bin/python3.11"),
            Path("/opt/homebrew/bin/python3"),
            Path("/usr/bin/python3"),
        ]
    )

    for binary_name in ("python3.11", "python3"):
        detected = detect_binary(binary_name)
        if detected:
            candidate_paths.append(Path(detected))

    seen: set[Path] = set()
    for candidate in candidate_paths:
        try:
            resolved = candidate.expanduser().resolve()
        except FileNotFoundError:
            continue
        if resolved in seen or not resolved.exists():
            continue
        seen.add(resolved)

        completed = subprocess.run(
            [
                str(resolved),
                "-c",
                "import json, sys; print(json.dumps({'prefix': sys.prefix, 'base_prefix': sys.base_prefix}))",
            ],
            env=sanitized_subprocess_env(),
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
        )
        if completed.returncode != 0:
            continue
        try:
            payload = json.loads(completed.stdout.strip())
        except json.JSONDecodeError:
            continue
        if payload.get("prefix") == payload.get("base_prefix"):
            return resolved

    raise RuntimeError(
        "could not locate a clean python3 interpreter for esp-idf tooling; "
        "set ESP32_PROBE_PYTHON3 explicitly"
    )


def ensure_python3_shim(context: RunContext) -> Path:
    python3 = locate_clean_python3()
    shim_dir = context.artifact_dir / "tool-shims"
    shim_dir.mkdir(parents=True, exist_ok=True)
    shim_path = shim_dir / "python3"
    shim_path.write_text(
        "\n".join(
            [
                "#!/bin/sh",
                "unset VIRTUAL_ENV",
                "unset VIRTUAL_ENV_DISABLE_PROMPT",
                "unset __PYVENV_LAUNCHER__",
                "unset PYTHONEXECUTABLE",
                "unset PYTHONHOME",
                "unset PYTHONPATH",
                f'exec "{python3}" "$@"',
                "",
            ]
        ),
        encoding="utf-8",
    )
    shim_path.chmod(0o755)
    return shim_dir


def firmware_env(
    context: RunContext,
    args: argparse.Namespace,
    provider_key: str,
    port: str,
) -> dict[str, str]:
    env = sanitized_subprocess_env()
    python3_shim_dir = ensure_python3_shim(context)
    env["ESPFLASH_SKIP_UPDATE_CHECK"] = "true"
    env["MCU"] = "esp32s3"
    env["WIFI_SSID"] = args.wifi_ssid or ""
    env["WIFI_PASS"] = args.wifi_password or ""
    env["OPENAI_API_KEY"] = provider_key
    env["OPENAI_MODEL"] = args.openai_model
    env["ESPFLASH_PORT"] = port
    env["PATH"] = f"{python3_shim_dir}:{env.get('PATH', '')}"
    return env


def parse_board_info(output: str, port: str) -> dict[str, object]:
    chip_match = re.search(r"Chip type:\s+(?P<chip>.+)", output)
    features_match = re.search(r"Features:\s+(?P<features>.+)", output)
    crystal_match = re.search(r"Crystal frequency:\s+(?P<crystal>.+)", output)
    usb_mode_match = re.search(r"USB mode:\s+(?P<usb_mode>.+)", output)
    mac_match = re.search(r"MAC:\s+(?P<mac>[0-9a-f:]{17})", output, flags=re.IGNORECASE)
    flash_match = re.search(r"Detected flash size:\s+(?P<flash>\S+)", output)
    psram_match = re.search(r"Embedded PSRAM\s+(?P<psram>\d+)MB", output)

    return {
        "port": port,
        "chip": chip_match.group("chip") if chip_match else None,
        "features": features_match.group("features") if features_match else None,
        "crystal": crystal_match.group("crystal") if crystal_match else None,
        "usb_mode": usb_mode_match.group("usb_mode") if usb_mode_match else None,
        "mac": mac_match.group("mac") if mac_match else None,
        "flash_size": flash_match.group("flash") if flash_match else None,
        "psram_mb": int(psram_match.group("psram")) if psram_match else None,
        "raw_probe": output,
    }


def host_usb_reset(context: RunContext, port: str) -> bool:
    python3 = locate_clean_python3()
    script = """
import json
import sys

port = sys.argv[1]

try:
    from serial.tools import list_ports
except Exception as error:
    print(json.dumps({"status": "skip", "reason": f"pyserial unavailable: {error}"}))
    raise SystemExit(0)

device = None
for candidate in list_ports.comports():
    if candidate.device == port:
        device = candidate
        break

if device is None or device.vid is None or device.pid is None:
    print(json.dumps({"status": "skip", "reason": "port missing usb vid/pid"}))
    raise SystemExit(0)

try:
    import usb.core
except Exception as error:
    print(json.dumps({"status": "skip", "reason": f"pyusb unavailable: {error}"}))
    raise SystemExit(0)

target = usb.core.find(
    idVendor=device.vid,
    idProduct=device.pid,
    serial_number=device.serial_number,
)
if target is None:
    target = usb.core.find(idVendor=device.vid, idProduct=device.pid)

if target is None:
    print(json.dumps({"status": "skip", "reason": "usb device not found"}))
    raise SystemExit(0)

target.reset()
print(
    json.dumps(
        {
            "status": "ok",
            "vid": device.vid,
            "pid": device.pid,
            "serial": device.serial_number,
        }
    )
)
"""
    completed = subprocess.run(
        [str(python3), "-c", script, port],
        cwd=REPO_ROOT,
        env=sanitized_subprocess_env(),
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
    )
    if completed.returncode != 0:
        context.emit(
            "[esp32-contract-probe] host usb reset helper failed; "
            + completed.stdout.strip()
        )
        return False

    output = completed.stdout.strip()
    if output:
        context.emit(f"[esp32-contract-probe] host usb reset: {output}")
    time.sleep(2)
    return '"status": "ok"' in output


def probe_board(context: RunContext, port: str) -> dict[str, object]:
    command = detect_esptool_command() + [
        "--chip",
        "esp32s3",
        "--port",
        port,
        "--before",
        "usb-reset",
        "flash-id",
    ]
    context.emit(
        f"[esp32-contract-probe] probing board on {port}: {' '.join(command)}"
    )
    if context.dry_run:
        return {
            "port": port,
            "chip": "ESP32-S3",
            "features": "Wi-Fi, BT 5 (LE), Dual Core + LP Core, 240MHz, Embedded PSRAM 8MB (AP_3v3)",
            "crystal": "40MHz",
            "usb_mode": "USB-Serial/JTAG",
            "mac": "00:00:00:00:00:00",
            "flash_size": "16MB",
            "psram_mb": 8,
            "raw_probe": "dry-run",
        }

    attempts: list[str] = []
    for attempt in range(1, 4):
        if attempt > 1:
            host_usb_reset(context, port)
        completed = subprocess.run(
            command,
            cwd=REPO_ROOT,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
        )
        attempts.append(f"[attempt {attempt}]\n{completed.stdout}".rstrip() + "\n")
        if completed.returncode == 0:
            board_log = context.artifact_dir / "board_probe.log"
            board_log.write_text("\n".join(attempts), encoding="utf-8")
            info = parse_board_info(completed.stdout, port)
            info["probe_log"] = display_path(board_log)
            return info
        if attempt < 3:
            time.sleep(2)

    board_log = context.artifact_dir / "board_probe.log"
    board_log.write_text("\n".join(attempts), encoding="utf-8")
    raise RuntimeError(f"board probe failed; see {display_path(board_log)}")


def collect_rust_stack_prep(context: RunContext, args: argparse.Namespace) -> RustStackPrep:
    port = detect_port(args.port)
    board = probe_board(context, port)
    tool_status = ensure_required_tools(RUST_STACK_TOOLS + ("espup", "cargo-espflash", "espflash"))
    missing_core = sorted(
        tool for tool in RUST_STACK_TOOLS if not tool_status.get(tool, False)
    )
    has_flash_tool = tool_status.get("cargo-espflash", False) or tool_status.get(
        "espflash",
        False,
    )
    error: str | None = None
    status = "pass"

    if missing_core:
        status = "fail"
        error = f"missing Rust core tools: {', '.join(missing_core)}"
    elif not tool_status.get("espup", False):
        status = "fail"
        error = "missing espup; Rust Xtensa toolchain bootstrap is not prepared"
    elif not has_flash_tool:
        status = "fail"
        error = "missing espflash or cargo-espflash"

    return RustStackPrep(
        port=port,
        board=board,
        tool_status=tool_status,
        status=status,
        error=error,
    )


def run_single_node_hardware_prep(context: RunContext, args: argparse.Namespace) -> int:
    try:
        port = detect_port(args.port)
        board = probe_board(context, port)
        tool_status = ensure_required_tools(PREP_REQUIRED_TOOLS)
        missing_tools = sorted(tool for tool, present in tool_status.items() if not present)
        provider_key = resolve_provider_key(args.provider, args.provider_api_key)
        wifi_ok = bool(args.wifi_ssid and args.wifi_password)
        provider_ok = bool(provider_key)

        context.emit(
            "MKT:BOARD:OK"
            f" port={board['port']}"
            f" chip={json.dumps(board['chip'])}"
            f" flash={json.dumps(board['flash_size'])}"
            f" psram_mb={board['psram_mb']}"
        )
        context.emit(
            f"MKT:WIFI:CONFIG_{'OK' if wifi_ok else 'MISSING'}"
        )
        context.emit(
            f"MKT:PROVIDER:CONFIG_{'OK' if provider_ok else 'MISSING'} provider={args.provider}"
        )
        if missing_tools:
            context.emit(
                f"MKT:TOOLS:MISSING tools={json.dumps(missing_tools)}"
            )
        else:
            context.emit("MKT:TOOLS:OK")
        status = "pass"
        error: str | None = None
        if missing_tools:
            status = "fail"
            error = f"missing required prep tools: {', '.join(missing_tools)}"
        elif not wifi_ok:
            status = "fail"
            error = "missing Wi-Fi credentials"
        elif not provider_ok and not args.allow_missing_provider_key:
            status = "fail"
            error = (
                f"missing provider API key for {args.provider}; set ESP32_PROBE_PROVIDER_API_KEY "
                "or the provider-specific environment variable"
            )

        if status == "pass":
            context.emit("MKT:HARDWARE:PREPARED mode=single-node")
        else:
            context.emit(f"MKT:HARDWARE:PREP_FAIL error={json.dumps(error)}")
        write_json(
            context.artifact_dir / "summary.json",
            {
                "mode": "single-node",
                "lane": "hardware-preflight",
                "status": status,
                "board": {
                    key: value
                    for key, value in board.items()
                    if key != "raw_probe"
                },
                "tool_status": tool_status,
                "wifi_configured": wifi_ok,
                "wifi_ssid": args.wifi_ssid,
                "provider": args.provider,
                "provider_key_configured": provider_ok,
                "error": error,
            },
        )
        return 0 if status == "pass" else 1
    except Exception as error:
        context.emit(f"MKT:HARDWARE:PREP_FAIL error={json.dumps(str(error))}")
        write_json(
            context.artifact_dir / "summary.json",
            {
                "mode": "single-node",
                "lane": "hardware-preflight",
                "status": "fail",
                "error": str(error),
            },
        )
        return 1


def run_single_node_rust_stack_prep(context: RunContext, args: argparse.Namespace) -> int:
    try:
        prep = collect_rust_stack_prep(context, args)

        context.emit(
            "MKT:BOARD:OK"
            f" port={prep.board['port']}"
            f" chip={json.dumps(prep.board['chip'])}"
            f" flash={json.dumps(prep.board['flash_size'])}"
            f" psram_mb={prep.board['psram_mb']}"
        )
        context.emit(
            "MKT:RUST_STACK:"
            + ("PREPARED" if prep.status == "pass" else "PREP_FAIL")
            + ("" if not prep.error else f" error={json.dumps(prep.error)}")
        )

        write_json(
            context.artifact_dir / "summary.json",
            {
                "mode": "single-node-rust-stack",
                "lane": "hardware-preflight",
                "status": prep.status,
                "board": {
                    key: value
                    for key, value in prep.board.items()
                    if key != "raw_probe"
                },
                "tool_status": prep.tool_status,
                "error": prep.error,
            },
        )
        return 0 if prep.status == "pass" else 1
    except Exception as error:
        context.emit(f"MKT:RUST_STACK:PREP_FAIL error={json.dumps(str(error))}")
        write_json(
            context.artifact_dir / "summary.json",
            {
                "mode": "single-node-rust-stack",
                "lane": "hardware-preflight",
                "status": "fail",
                "error": str(error),
            },
        )
        return 1


def run_monitored_command(
    context: RunContext,
    command: Sequence[str],
    cwd: Path,
    env: dict[str, str],
    transcript_path: Path,
    required_markers: Sequence[str],
    timeout_secs: int,
) -> dict[str, object]:
    if context.dry_run:
        transcript_path.write_text(
            "\n".join(required_markers) + "\n",
            encoding="utf-8",
        )
        return {
            "command": list(command),
            "cwd": display_path(cwd),
            "transcript": display_path(transcript_path),
            "exit_code": 0,
            "dry_run": True,
        }

    with transcript_path.open("w", encoding="utf-8") as transcript:
        process = subprocess.Popen(
            list(command),
            cwd=cwd,
            env=env,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
            bufsize=1,
            errors="replace",
        )
        start = time.monotonic()
        marker_text = ""
        last_line = ""
        pass_seen = False

        try:
            assert process.stdout is not None
            while True:
                if time.monotonic() - start > timeout_secs:
                    raise TimeoutError(
                        f"timed out after {timeout_secs}s waiting for hardware probe markers"
                    )

                line = process.stdout.readline()
                if line:
                    transcript.write(line)
                    transcript.flush()
                    last_line = line.rstrip("\n")
                    if "MKT:" in line:
                        stripped = line.rstrip()
                        context.emit(stripped)
                        marker_text += stripped + "\n"
                        try:
                            validate_required_markers(
                                extract_markers(marker_text),
                                required_markers,
                            )
                            pass_seen = True
                            break
                        except MarkerValidationError:
                            pass
                    continue

                if process.poll() is not None:
                    break
                time.sleep(0.1)
        finally:
            if process.poll() is None:
                process.send_signal(signal.SIGINT)
                try:
                    process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait(timeout=5)

        exit_code = process.returncode if process.returncode is not None else 0
        if not pass_seen:
            if exit_code != 0:
                raise RuntimeError(
                    f"hardware probe command exited with code {exit_code}; last output: {last_line}"
                )
            validate_required_markers(extract_markers(transcript_path.read_text(encoding="utf-8")), required_markers)

        return {
            "command": list(command),
            "cwd": display_path(cwd),
            "transcript": display_path(transcript_path),
            "exit_code": exit_code,
        }


def run_single_node_rust_stack_hardware(context: RunContext, args: argparse.Namespace) -> int:
    try:
        if args.provider != "openai":
            raise RuntimeError(
                "single-node-rust-stack hardware lane currently supports provider=openai only"
            )

        prep = collect_rust_stack_prep(context, args)
        context.emit(
            "MKT:BOARD:OK"
            f" port={prep.board['port']}"
            f" chip={json.dumps(prep.board['chip'])}"
            f" flash={json.dumps(prep.board['flash_size'])}"
            f" psram_mb={prep.board['psram_mb']}"
        )
        context.emit(
            "MKT:RUST_STACK:"
            + ("PREPARED" if prep.status == "pass" else "PREP_FAIL")
            + ("" if not prep.error else f" error={json.dumps(prep.error)}")
        )
        if prep.status != "pass":
            write_json(
                context.artifact_dir / "summary.json",
                {
                    "mode": "single-node-rust-stack",
                    "lane": "hardware",
                    "status": "fail",
                    "board": {
                        key: value
                        for key, value in prep.board.items()
                        if key != "raw_probe"
                    },
                    "tool_status": prep.tool_status,
                    "error": prep.error,
                },
            )
            return 1

        provider_key = resolve_provider_key(args.provider, args.provider_api_key)
        if not args.wifi_ssid or not args.wifi_password:
            raise RuntimeError("missing Wi-Fi credentials for rust-stack hardware run")
        if not provider_key:
            raise RuntimeError("missing provider API key for rust-stack hardware run")

        env = firmware_env(context, args, provider_key, prep.port)
        serial_log = context.artifact_dir / "serial.log"
        host_usb_reset(context, prep.port)
        command = [
            "cargo",
            "espflash",
            "flash",
            "--release",
            "--target",
            "xtensa-esp32s3-espidf",
            "--chip",
            "esp32s3",
            "--port",
            prep.port,
            "--before",
            "usb-reset",
            "--non-interactive",
            "--skip-update-check",
            "--monitor",
            "--monitor-baud",
            "115200",
        ]

        context.emit(
            "[esp32-contract-probe] flashing rust firmware: "
            + " ".join(command)
        )
        command_result = run_monitored_command(
            context,
            command,
            FIRMWARE_RUST_ROOT,
            env,
            serial_log,
            RUST_STACK_MARKERS,
            args.monitor_timeout_secs,
        )
        report = write_marker_report(
            context,
            "single-node-rust-stack",
            RUST_STACK_MARKERS,
            serial_log,
        )
        write_json(
            context.artifact_dir / "summary.json",
            {
                "mode": "single-node-rust-stack",
                "lane": "hardware",
                "status": "pass",
                "board": {
                    key: value
                    for key, value in prep.board.items()
                    if key != "raw_probe"
                },
                "provider": args.provider,
                "model": args.openai_model,
                "marker_report": report,
                "command": command_result,
            },
        )
        return 0
    except Exception as error:
        context.emit(f"MKT:RUST_STACK:FAIL error={json.dumps(str(error))}")
        write_json(
            context.artifact_dir / "summary.json",
            {
                "mode": "single-node-rust-stack",
                "lane": "hardware",
                "status": "fail",
                "error": str(error),
            },
        )
        return 1


def run_negative(context: RunContext) -> int:
    broken_transcript = context.artifact_dir / "broken.log"
    broken_transcript.write_text(
        "\n".join(
            [
                "MKT:HOST_CORE:FACTORY_OK",
                "MKT:HOST_CORE:RUNTIME_OK",
                "MKT:HOST_CORE:PASS",
            ]
        )
        + "\n",
        encoding="utf-8",
    )
    try:
        write_marker_report(
            context,
            "negative-host-core",
            HOST_CORE_MARKERS,
            broken_transcript,
        )
    except MarkerValidationError as error:
        context.emit(f"MKT:NEGATIVE:PASS error={json.dumps(str(error))}")
        write_json(
            context.artifact_dir / "summary.json",
            {
                "mode": "negative",
                "status": "pass",
                "error": str(error),
            },
        )
        return 0

    context.emit("MKT:NEGATIVE:FAIL error=\"parser accepted invalid marker sequence\"")
    write_json(
        context.artifact_dir / "summary.json",
        {
            "mode": "negative",
            "status": "fail",
            "error": "parser accepted invalid marker sequence",
        },
    )
    return 1


def main(argv: Sequence[str] | None = None) -> int:
    args = parse_args(argv)
    context = ensure_run_context(args.mode, args.artifact_root, args.dry_run)

    if args.mode == "host-core-check":
        return run_host_core_check(context)
    if args.mode == "single-node":
        if args.host_sim:
            return run_single_node_host_sim(context, args.provider)
        return run_single_node_hardware_prep(context, args)
    if args.mode == "single-node-rust-stack":
        if args.host_sim:
            raise RuntimeError("single-node-rust-stack does not support --host-sim")
        return run_single_node_rust_stack_hardware(context, args)
    if args.mode == "negative":
        return run_negative(context)
    raise AssertionError(f"unsupported mode: {args.mode}")


if __name__ == "__main__":
    sys.exit(main())
