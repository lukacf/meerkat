#!/usr/bin/env python3

from __future__ import annotations

import importlib.util
import json
import os
import pathlib
import re
import subprocess
import sys
import threading
import time

import serial


REPO = pathlib.Path(__file__).resolve().parents[3]
PROBE_PATH = REPO / "scripts" / "live_smoke" / "esp32-contract-probe" / "esp32_contract_probe.py"
DEFAULT_DEVICE_PEER_ID = "ed25519:/RckOFqgx1tk+3jNYC+h2ZH96/drE8WO1wLqyDXp9hg="
DEFAULT_DEVICE_MAC = "28:37:2f:88:d5:48"


def load_probe():
    spec = importlib.util.spec_from_file_location("probe", PROBE_PATH)
    module = importlib.util.module_from_spec(spec)
    sys.modules["probe"] = module
    assert spec.loader is not None
    spec.loader.exec_module(module)
    return module


def main() -> int:
    probe = load_probe()
    port = os.environ.get("ESP32_PORT", "/dev/cu.usbmodem101")
    monitor_port = os.environ.get("ESP32_MONITOR_PORT", port)
    wifi_ssid = os.environ["ESP32_WIFI_SSID"]
    wifi_password = os.environ["ESP32_WIFI_PASSWORD"]
    provider_api_key = os.environ["OPENAI_API_KEY"]
    openai_model = os.environ.get("OPENAI_MODEL", "gpt-5.4-mini")
    host_ip = os.environ.get("ESP32_HOST_IP", "192.168.0.197")
    host_port = os.environ.get("ESP32_HOST_PORT", "4220")
    host_listen = f"{host_ip}:{host_port}"
    device_ip_hint = os.environ.get("ESP32_DEVICE_IP_HINT")
    skip_build_flash = os.environ.get("ESP32_SKIP_BUILD_FLASH") == "1"
    force_reset = os.environ.get("ESP32_FORCE_RESET") == "1"
    device_peer_id = os.environ.get("ESP32_PEER_ID", DEFAULT_DEVICE_PEER_ID)
    device_mac = os.environ.get("ESP32_MAC", DEFAULT_DEVICE_MAC).lower()

    class Args:
        pass

    args = Args()
    args.wifi_ssid = wifi_ssid
    args.wifi_password = wifi_password
    args.openai_model = openai_model
    args.provider = "openai"
    args.provider_api_key = provider_api_key

    artifact_dir = (
        REPO
        / "artifacts"
        / "embedded-esp32"
        / "phase-0"
        / "comms-orchestrator"
        / time.strftime("%Y%m%d-%H%M%S", time.gmtime())
    )
    artifact_dir.mkdir(parents=True, exist_ok=True)
    serial_log = artifact_dir / "serial.log"
    host_log = artifact_dir / "host.log"
    summary_path = artifact_dir / "summary.json"

    ctx = probe.ensure_run_context(
        "comms-orchestrator",
        pathlib.Path("/tmp/esp32-probe-artifacts"),
        False,
    )
    probe.wait_for_serial_port(port)
    env = probe.firmware_env(ctx, args, provider_api_key, port)
    env["ENABLE_COMMS"] = "1"
    env["HOST_PEER_NAME"] = "phase0-host"
    env["HOST_PEER_ADDR"] = f"tcp://{host_listen}"
    env["COMMS_LISTEN_PORT"] = "4210"
    device_stream = os.environ.get("OPENAI_STREAM_DEVICE", os.environ.get("OPENAI_STREAM", "0"))
    host_stream = os.environ.get("OPENAI_STREAM_HOST", os.environ.get("OPENAI_STREAM", "0"))
    env["OPENAI_STREAM_DEVICE"] = device_stream
    if os.environ.get("ESP32_SKIP_SINGLE_NODE", "1") == "1":
        env["SKIP_SINGLE_NODE"] = "1"

    if not skip_build_flash:
        probe.host_usb_reset(ctx, port)
        probe.wait_for_serial_port(port)

        build = probe.run_command_with_env(
            ctx,
            probe.CommandSpec(
                label="build_rust_firmware",
                command=(
                    "rustup",
                    "run",
                    "esp",
                    "cargo",
                    "build",
                    "--release",
                    "--target",
                    "xtensa-esp32s3-espidf",
                    "-Zbuild-std=std,panic_abort",
                    "--ignore-rust-version",
                ),
                cwd=probe.FIRMWARE_RUST_ROOT,
            ),
            1,
            env,
        )
        print("BUILD", build["exit_code"])

        image = (
            pathlib.Path(env["CARGO_TARGET_DIR"])
            / "xtensa-esp32s3-espidf"
            / "release"
            / "esp32-contract-probe-fw"
        )

        flash = probe.run_monitored_command(
            ctx,
            [
                "espflash",
                "flash",
                str(image),
                "--chip",
                "esp32s3",
                "--port",
                port,
                "--before",
                "usb-reset",
                "--non-interactive",
                "--skip-update-check",
            ],
            REPO,
            env,
            artifact_dir / "flash.log",
            (),
            120,
        )
        print("FLASH", flash)

        time.sleep(2)
        active_port = probe.wait_for_serial_port(port)
        probe.host_usb_reset(ctx, active_port)
        probe.wait_for_serial_port(port)

    else:
        print("SKIP_BUILD_FLASH", 1)
        if force_reset:
            probe.host_usb_reset(ctx, port)
        probe.wait_for_serial_port(port)

    active_port = monitor_port
    state = {
        "wifi_ip": None,
        "peer_id": device_peer_id,
        "comms_waiting": False,
        "comms_ready": False,
        "comms_pass": False,
        "host_pass": False,
        "host_started": False,
        "host_error": None,
    }
    lock = threading.Lock()
    saw_serial_output = False

    def run_host(peer_id: str, wifi_ip: str) -> None:
        host_cmd = [
            "cargo",
            "run",
            "--quiet",
            "--manifest-path",
            str(REPO / "scripts" / "live_smoke" / "esp32-contract-probe" / "host-meerkat" / "Cargo.toml"),
            "--",
            "--peer-name",
            "esp32-probe",
            "--peer-id",
            peer_id,
            "--peer-addr",
            f"tcp://{wifi_ip}:4210",
            "--listen-addr",
            host_listen,
            "--exchanges",
            os.environ.get("ESP32_EXCHANGES", "15"),
        ]
        host_env = os.environ.copy()
        host_env["OPENAI_API_KEY"] = provider_api_key
        host_env["OPENAI_MODEL"] = openai_model
        host_env["OPENAI_STREAM_HOST"] = host_stream
        lines: list[str] = []
        with host_log.open("w", encoding="utf-8") as sink:
            proc = subprocess.Popen(
                host_cmd,
                cwd=REPO,
                env=host_env,
                stdout=subprocess.PIPE,
                stderr=subprocess.STDOUT,
                text=True,
                bufsize=1,
            )
            assert proc.stdout is not None
            for line in proc.stdout:
                lines.append(line)
                sink.write(line)
                sink.flush()
                sys.stdout.write(line)
                sys.stdout.flush()
            cp_returncode = proc.wait()
        host_output = "".join(lines)
        with lock:
            if cp_returncode != 0:
                state["host_error"] = f"host peer failed with {cp_returncode}"
            elif "MKT:HOST_MEERKAT:PASS" in host_output:
                state["host_pass"] = True
            else:
                state["host_error"] = "host meerkat finished without PASS marker"

    def arp_lookup_ip() -> str | None:
        cp = subprocess.run(
            ["arp", "-na"],
            cwd=REPO,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
        )
        if cp.returncode != 0:
            return None
        for line in cp.stdout.splitlines():
            lower = line.lower()
            if device_mac in lower:
                match = re.search(r"\(([^)]+)\)", line)
                if match:
                    return match.group(1)
        return None

    with serial.Serial(active_port, 115200, timeout=0.25) as ser, serial_log.open(
        "w", encoding="utf-8"
    ) as slog:
        deadline = time.monotonic() + 300
        fallback_host_start = time.monotonic() + 20
        host_pass_grace_deadline = None
        while time.monotonic() < deadline:
            chunk = ser.read(4096)
            if chunk:
                saw_serial_output = True
                text = chunk.decode("utf-8", errors="replace")
                slog.write(text)
                slog.flush()
                sys.stdout.write(text)
                sys.stdout.flush()
                for raw in text.splitlines():
                    line = raw.strip()
                    if line.startswith("MKT:WIFI:OK"):
                        match = re.search(r'ip=(".*?")', line)
                        if match:
                            with lock:
                                state["wifi_ip"] = json.loads(match.group(1))
                    elif line.startswith("MKT:COMMS:LISTENING"):
                        match = re.search(r'peer_id=(".*?")', line)
                        if match:
                            with lock:
                                state["peer_id"] = json.loads(match.group(1))
                    elif line.startswith("MKT:COMMS:WAITING"):
                        with lock:
                            state["comms_waiting"] = True
                    elif line.startswith("MKT:COMMS:READY"):
                        with lock:
                            state["comms_ready"] = True
                    elif line.startswith("MKT:COMMS:PASS"):
                        with lock:
                            state["comms_pass"] = True

            with lock:
                if not state["wifi_ip"]:
                    arp_ip = arp_lookup_ip()
                    if arp_ip:
                        state["wifi_ip"] = arp_ip
                    elif device_ip_hint:
                        state["wifi_ip"] = device_ip_hint
                if (
                    (
                        state["comms_ready"]
                        or state["comms_waiting"]
                        or time.monotonic() >= fallback_host_start
                    )
                    and state["wifi_ip"]
                    and state["peer_id"]
                    and not state["host_started"]
                ):
                    threading.Thread(
                        target=run_host,
                        args=(state["peer_id"], state["wifi_ip"]),
                        daemon=True,
                    ).start()
                    state["host_started"] = True

                if state["host_error"]:
                    raise SystemExit(state["host_error"])

                if state["host_pass"] and state["comms_pass"]:
                    break

                if state["host_pass"] and not saw_serial_output:
                    if host_pass_grace_deadline is None:
                        host_pass_grace_deadline = time.monotonic() + 3
                    elif time.monotonic() >= host_pass_grace_deadline:
                        break
            time.sleep(0.1)
        else:
            raise SystemExit(f"timed out waiting for comms pass; state={state}")

    summary = {
        "status": "pass",
        "wifi_ip": state["wifi_ip"],
        "peer_id": state["peer_id"],
        "host_listen": host_listen,
        "serial_log": str(serial_log),
        "host_log": str(host_log),
        "saw_serial_output": saw_serial_output,
        "comms_pass": state["comms_pass"],
        "host_pass": state["host_pass"],
    }
    summary_path.write_text(json.dumps(summary, indent=2) + "\n", encoding="utf-8")
    print("SUMMARY", json.dumps(summary))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
