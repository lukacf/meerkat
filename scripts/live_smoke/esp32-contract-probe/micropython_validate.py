#!/usr/bin/env python3

from __future__ import annotations

import json
import os
import pathlib
import re
import shutil
import socket
import subprocess
import sys
import threading
import time
from types import SimpleNamespace
from urllib.parse import urlparse
from glob import glob

import serial

import esp32_contract_probe as probe


REPO = pathlib.Path(__file__).resolve().parents[3]
HOST_MANIFEST = (
    REPO
    / "scripts"
    / "live_smoke"
    / "esp32-contract-probe"
    / "host-meerkat"
    / "Cargo.toml"
)


class SerialState:
    def __init__(self) -> None:
        self.peer_id: str | None = None
        self.peer_addr: str | None = None
        self.wifi_ip: str | None = None
        self.serial_port: int | None = None
        self.ready = False
        self.comms_pass = False
        self.serial_ready = False
        self.serial_pass = False
        self.serial_results = 0
        self.micropy_init_ok = False
        self.py_calls = 0
        self.lock = threading.Lock()

    def update(self, text: str) -> None:
        with self.lock:
            if "MKT:MICROPY:INIT_OK" in text:
                self.micropy_init_ok = True
            self.py_calls += text.count("MKT:MICROPY:COMPLETED is_error=false")
            if "MKT:COMMS:PASS" in text:
                self.comms_pass = True
            if "MKT:SERIAL:READY" in text:
                self.serial_ready = True
            if "MKT:SERIAL:PASS" in text:
                self.serial_pass = True
            self.serial_results += text.count("MKT:SERIAL:RUN_RESULT")
            for raw in text.splitlines():
                line = raw.strip()
                if line.startswith("MKT:COMMS:LISTENING"):
                    match = re.search(r'peer_id=(".*?")', line)
                    if match:
                        self.peer_id = json.loads(match.group(1))
                elif line.startswith("MKT:COMMS:READY"):
                    self.ready = True
                    match = re.search(r'addr=(".*?")', line)
                    if match:
                        self.peer_addr = json.loads(match.group(1))
                elif line.startswith("MKT:NETMARKER:READY"):
                    match = re.search(r'wifi_ip=(".*?")', line)
                    if match:
                        self.wifi_ip = json.loads(match.group(1))
                elif line.startswith("MKT:WIFI:OK"):
                    match = re.search(r'ip=(".*?")', line)
                    if match:
                        self.wifi_ip = json.loads(match.group(1))
                elif line.startswith("MKT:SERIAL:READY"):
                    match = re.search(r'port=(\d+)', line)
                    if match:
                        self.serial_port = int(match.group(1))


def resolve_wifi_build_inputs() -> tuple[str, str]:
    wifi_ssid = os.environ.get("WIFI_SSID")
    if not wifi_ssid:
        wifi_ssid = os.environ.get("ESP32_PROBE_WIFI_SSID")
    if not wifi_ssid:
        raise SystemExit(
            "missing Wi-Fi SSID; set WIFI_SSID or ESP32_PROBE_WIFI_SSID before build/flash"
        )

    wifi_password = os.environ.get("WIFI_PASS")
    if wifi_password is None:
        wifi_password = os.environ.get("ESP32_PROBE_WIFI_PASSWORD")
    if wifi_password is None:
        wifi_password = ""

    return wifi_ssid, wifi_password


def resolve_udp_port() -> int:
    return int(os.environ.get("ESP32_UDP_PORT", "42424"))


def resolve_port(explicit: str | None) -> str:
    if explicit and pathlib.Path(explicit).exists():
        return explicit
    candidates = sorted(glob("/dev/cu.usbmodem*")) or sorted(glob("/dev/tty.usbmodem*"))
    if not candidates:
        raise SystemExit(f"no ESP32 serial device found (requested={explicit!r})")
    return candidates[-1]


def resolve_host_listen() -> str:
    explicit = os.environ.get("ESP32_HOST_LISTEN")
    if explicit:
        return explicit
    sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    try:
        sock.connect(("8.8.8.8", 80))
        ip = sock.getsockname()[0]
    finally:
        sock.close()
    return f"{ip}:4220"


def resolve_host_mode() -> str:
    return os.environ.get("ESP32_HOST_MODE", "serial").strip().lower() or "serial"


def serial_prompt(index: int) -> str:
    return (
        "Run micropython exactly once with this code and then answer using the required format:\n"
        f"print('serial turn {index:02d}')\n"
        f"result = {index} * 7"
    )


def build_and_flash(
    artifact_dir: pathlib.Path,
    port: str,
    host_listen: str,
    host_mode: str,
) -> None:
    if os.environ.get("ESP32_SKIP_BUILD_FLASH", "0") == "1":
        print("MKT:VALIDATOR:SKIP_BUILD_FLASH", flush=True)
        return

    provider_key = os.environ["OPENAI_API_KEY"]
    wifi_ssid, wifi_password = resolve_wifi_build_inputs()
    args = SimpleNamespace(
        wifi_ssid=wifi_ssid,
        wifi_password=wifi_password,
        openai_model=os.environ.get("OPENAI_MODEL", "gpt-5.4-mini"),
    )
    context = SimpleNamespace(
        artifact_dir=artifact_dir,
        dry_run=False,
        emit=lambda line: print(line, flush=True),
    )

    probe.wait_for_serial_port(port)
    env = probe.firmware_env(context, args, provider_key, port)
    target_dir = pathlib.Path("/tmp") / f"esp32v-{artifact_dir.name}"
    shutil.rmtree(target_dir, ignore_errors=True)
    env["CARGO_TARGET_DIR"] = str(target_dir)
    env["SKIP_SINGLE_NODE"] = "1"
    env["HOST_INPUT_MODE"] = host_mode
    if host_mode == "comms":
        env["ENABLE_COMMS"] = "1"
        env["HOST_PEER_NAME"] = "phase0-host"
        env["HOST_PEER_ADDR"] = f"tcp://{host_listen}"
        env["COMMS_LISTEN_PORT"] = "4210"
    else:
        env["ENABLE_COMMS"] = "0"
    env["OPENAI_STREAM_DEVICE"] = os.environ.get(
        "OPENAI_STREAM_DEVICE", os.environ.get("OPENAI_STREAM", "0")
    )

    probe.host_usb_reset(context, port)
    active_port = probe.wait_for_serial_port(port)
    build_spec = probe.CommandSpec(
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
    )
    probe.run_command_with_env(context, build_spec, 1, env)

    image_path = (
        pathlib.Path(env["CARGO_TARGET_DIR"])
        / "xtensa-esp32s3-espidf"
        / "release"
        / "esp32-contract-probe-fw"
    )
    if not image_path.exists():
        raise SystemExit(f"built firmware image missing: {image_path}")

    flash_spec = probe.CommandSpec(
        label="flash_rust_firmware",
        command=(
            "espflash",
            "flash",
            str(image_path),
            "--chip",
            "esp32s3",
            "--port",
            active_port,
            "--before",
            "usb-reset",
            "--after",
            "hard-reset",
            "--non-interactive",
        ),
        cwd=probe.FIRMWARE_RUST_ROOT,
    )
    probe.run_command_with_env(context, flash_spec, 2, env)
    probe.wait_for_serial_port(active_port)


def monitor_udp(
    udp_log: pathlib.Path,
    state: SerialState,
    stop_event: threading.Event,
    udp_port: int,
) -> None:
    sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    sock.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    try:
        sock.bind(("0.0.0.0", udp_port))
    except OSError as error:
        with udp_log.open("w", encoding="utf-8") as sink:
            sink.write(f"UDP monitor disabled on {udp_port}: {error}\n")
        sys.stdout.write(
            f"MKT:VALIDATOR:UDP_DISABLED port={udp_port} error={error}\n"
        )
        sys.stdout.flush()
        sock.close()
        return
    sock.settimeout(0.2)
    with udp_log.open("w", encoding="utf-8") as sink:
        while not stop_event.is_set():
            try:
                payload, addr = sock.recvfrom(4096)
            except socket.timeout:
                continue
            text = payload.decode("utf-8", errors="replace").strip()
            if not text:
                continue
            line = f"{addr[0]} {text}\n"
            sink.write(line)
            sink.flush()
            state.update(text)
            sys.stdout.write(f"MKT:VALIDATOR:UDP {line}")
            sys.stdout.flush()
    sock.close()


def monitor_serial(
    ser: serial.Serial,
    serial_log: pathlib.Path,
    state: SerialState,
    stop_event: threading.Event,
) -> None:
    with serial_log.open("w", encoding="utf-8") as sink:
        while not stop_event.is_set():
            chunk = ser.read(4096)
            if chunk:
                text = chunk.decode("utf-8", errors="replace")
                sink.write(text)
                sink.flush()
                sys.stdout.write(text)
                sys.stdout.flush()
                state.update(text)
            else:
                time.sleep(0.05)


def reset_and_monitor(
    port: str, serial_log: pathlib.Path, state: SerialState, host_mode: str
) -> tuple[serial.Serial, threading.Event, threading.Thread]:
    last_error: Exception | None = None
    for attempt in range(1, 4):
        try:
            print(f"MKT:VALIDATOR:RESET_ATTEMPT attempt={attempt}", flush=True)
            ser = serial.Serial(port, 115200, timeout=0.2)
            ser.dtr = False
            ser.rts = True
            time.sleep(0.15)
            ser.reset_input_buffer()
            ser.reset_output_buffer()
            ser.dtr = True
            ser.rts = False
            time.sleep(0.2)

            stop_event = threading.Event()
            thread = threading.Thread(
                target=monitor_serial,
                args=(ser, serial_log, state, stop_event),
                daemon=True,
            )
            thread.start()

            deadline = time.time() + 35
            while time.time() < deadline:
                with state.lock:
                    if host_mode == "serial" and state.serial_ready:
                        print(
                            "MKT:VALIDATOR:DEVICE_READY mode=serial",
                            flush=True,
                        )
                        return ser, stop_event, thread
                    if (
                        host_mode == "comms"
                        and state.ready
                        and state.peer_id
                        and state.peer_addr
                    ):
                        print(
                            f"MKT:VALIDATOR:DEVICE_READY peer_id={state.peer_id} peer_addr={state.peer_addr}",
                            flush=True,
                        )
                        return ser, stop_event, thread
                time.sleep(0.1)

            stop_event.set()
            thread.join(timeout=2)
            ser.close()
            if host_mode == "serial":
                last_error = RuntimeError(
                    f"attempt {attempt}: device did not reach SERIAL:READY on {port}"
                )
            else:
                last_error = RuntimeError(
                    f"attempt {attempt}: device did not reach COMMS:READY on {port}; peer_id={state.peer_id} peer_addr={state.peer_addr}"
                )
        except Exception as exc:
            last_error = exc
        time.sleep(0.5)

    raise SystemExit(str(last_error))


def run_host(
    artifact_dir: pathlib.Path,
    peer_id: str,
    peer_addr: str,
    host_listen: str,
    exchanges: int,
    timeout_secs: int,
) -> tuple[str, int, bool]:
    host_log = artifact_dir / "host.log"
    host_cmd = [
        "cargo",
        "run",
        "--quiet",
        "--manifest-path",
        str(HOST_MANIFEST),
        "--",
        "--peer-name",
        "esp32-probe",
        "--peer-id",
        peer_id,
        "--peer-addr",
        peer_addr,
        "--listen-addr",
        host_listen,
        "--exchanges",
        str(exchanges),
    ]
    host_env = os.environ.copy()
    host_env["OPENAI_API_KEY"] = os.environ["OPENAI_API_KEY"]
    host_env["OPENAI_MODEL"] = os.environ.get("OPENAI_MODEL", "gpt-5.4-mini")
    host_env["OPENAI_STREAM_HOST"] = os.environ.get("OPENAI_STREAM_HOST", "0")
    host_env["ESP32_SCENARIO"] = "micropython"
    lines: list[str] = []
    timed_out = False
    pass_seen = False
    pass_grace_deadline: float | None = None
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
        start = time.time()
        while True:
            line = proc.stdout.readline()
            if line:
                lines.append(line)
                sink.write(line)
                sink.flush()
                sys.stdout.write(line)
                sys.stdout.flush()
                if "MKT:HOST_MEERKAT:PASS" in line and not pass_seen:
                    pass_seen = True
                    pass_grace_deadline = time.time() + 5
            elif proc.poll() is not None:
                return "".join(lines), proc.wait(), timed_out

            if pass_grace_deadline is not None and time.time() > pass_grace_deadline:
                proc.terminate()
                try:
                    proc.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    proc.kill()
                    proc.wait(timeout=5)
                return "".join(lines), 0, timed_out

            if time.time() - start > timeout_secs:
                timed_out = True
                proc.kill()
                try:
                    proc.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    proc.terminate()
                timeout_line = (
                    f"MKT:VALIDATOR:HOST_TIMEOUT seconds={timeout_secs}\n"
                )
                lines.append(timeout_line)
                sink.write(timeout_line)
                sink.flush()
                sys.stdout.write(timeout_line)
                sys.stdout.flush()
                return "".join(lines), proc.returncode or 124, timed_out


def run_serial_host(
    artifact_dir: pathlib.Path,
    host_ip: str,
    host_port: int,
    state: SerialState,
    exchanges: int,
    timeout_secs: int,
) -> tuple[str, int, bool]:
    host_log = artifact_dir / "host.log"
    lines: list[str] = []
    timed_out = False
    start = time.time()

    def log(line: str) -> None:
        lines.append(line)
        with host_log.open("a", encoding="utf-8") as sink:
            sink.write(line)
        sys.stdout.write(line)
        sys.stdout.flush()

    host_log.write_text("", encoding="utf-8")
    sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    sock.setsockopt(socket.SOL_SOCKET, socket.SO_BROADCAST, 1)

    try:
        for index in range(1, exchanges + 1):
            prompt = serial_prompt(index)
            payload = json.dumps({"cmd": "prompt", "text": prompt}) + "\n"
            with state.lock:
                target_results = state.serial_results + 1
            sock.sendto(payload.encode("utf-8"), ("255.255.255.255", host_port))
            log(
                f"MKT:HOST_SERIAL:PROMPT index={index} target=255.255.255.255:{host_port} device_ip={host_ip} text={json.dumps(prompt)}\n"
            )

            deadline = time.time() + timeout_secs
            while time.time() < deadline:
                with state.lock:
                    if state.serial_results >= target_results:
                        break
                time.sleep(0.1)
            else:
                timed_out = True
                log(f"MKT:HOST_SERIAL:TIMEOUT index={index} seconds={timeout_secs}\n")
                return "".join(lines), 124, timed_out

        sock.sendto(b'{"cmd":"stop"}\n', ("255.255.255.255", host_port))
        log("MKT:HOST_SERIAL:STOP\n")

        deadline = time.time() + max(20, timeout_secs)
        while time.time() < deadline:
            with state.lock:
                if state.serial_pass:
                    log(
                        f"MKT:HOST_SERIAL:PASS prompts={exchanges} elapsed_ms={int((time.time() - start) * 1000)}\n"
                    )
                    return "".join(lines), 0, timed_out
            time.sleep(0.1)

        timed_out = True
        log(f"MKT:HOST_SERIAL:TIMEOUT stop_wait seconds={max(20, timeout_secs)}\n")
        return "".join(lines), 124, timed_out
    finally:
        sock.close()


def wait_for_device_ready(
    state: SerialState,
    timeout_secs: int,
    label: str,
    host_mode: str,
) -> None:
    deadline = time.time() + timeout_secs
    while time.time() < deadline:
        with state.lock:
            if host_mode == "serial" and state.serial_ready:
                print(
                    f"MKT:VALIDATOR:DEVICE_READY via={label} mode=serial",
                    flush=True,
                )
                return
            if state.ready and state.peer_id and state.peer_addr:
                print(
                    f"MKT:VALIDATOR:DEVICE_READY via={label} peer_id={state.peer_id} peer_addr={state.peer_addr}",
                    flush=True,
                )
                return
        time.sleep(0.1)
    if host_mode == "serial":
        raise SystemExit(f"device did not reach SERIAL:READY via {label}")
    raise SystemExit(
        f"device did not reach COMMS:READY via {label}; peer_id={state.peer_id} peer_addr={state.peer_addr}"
    )


def peer_tcp_connectivity(peer_addr: str, timeout_secs: float = 3.0) -> tuple[bool, str]:
    parsed = urlparse(peer_addr)
    if parsed.scheme != "tcp" or not parsed.hostname or parsed.port is None:
        return False, f"unsupported_peer_addr:{peer_addr}"

    sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    sock.settimeout(timeout_secs)
    try:
        sock.connect((parsed.hostname, parsed.port))
    except Exception as error:
        return False, f"{type(error).__name__}: {error}"
    finally:
        sock.close()
    return True, "ok"


def main() -> int:
    port = resolve_port(os.environ.get("ESP32_PORT"))
    host_mode = resolve_host_mode()
    host_listen = resolve_host_listen() if host_mode == "comms" else ""
    exchanges = int(os.environ.get("ESP32_EXCHANGES", "10"))
    default_host_timeout_secs = max(300, exchanges * 15)
    host_timeout_secs = int(
        os.environ.get("ESP32_HOST_TIMEOUT_SECS", str(default_host_timeout_secs))
    )
    ready_timeout_secs = int(os.environ.get("ESP32_READY_TIMEOUT_SECS", "60"))
    udp_port = resolve_udp_port()
    skip_reset = os.environ.get("ESP32_SKIP_RESET", "0") == "1"
    artifact_dir = (
        REPO
        / "artifacts"
        / "embedded-esp32"
        / "phase-0"
        / "micropython-serial-validate"
        / time.strftime("%Y%m%d-%H%M%S", time.gmtime())
    )
    artifact_dir.mkdir(parents=True, exist_ok=True)
    serial_log = artifact_dir / "serial.log"
    udp_log = artifact_dir / "udp.log"

    serial_state = SerialState()
    udp_stop = threading.Event()
    udp_thread = threading.Thread(
        target=monitor_udp,
        args=(udp_log, serial_state, udp_stop, udp_port),
        daemon=True,
    )
    udp_thread.start()

    build_and_flash(artifact_dir, port, host_listen, host_mode)

    ser = None
    stop_event = None
    thread = None
    if skip_reset:
        serial_log.write_text("", encoding="utf-8")
        print(
            f"MKT:VALIDATOR:PASSIVE_WAIT port={port} timeout_secs={ready_timeout_secs}",
            flush=True,
        )
        wait_for_device_ready(serial_state, ready_timeout_secs, "udp", host_mode)
    else:
        ser, stop_event, thread = reset_and_monitor(
            port, serial_log, serial_state, host_mode
        )

    peer_addr = serial_state.peer_addr or ""
    peer_tcp_ok = False
    peer_tcp_detail = "skipped"
    if host_mode == "comms":
        peer_tcp_ok, peer_tcp_detail = peer_tcp_connectivity(peer_addr)
        print(
            f"MKT:VALIDATOR:PEER_TCP peer_addr={peer_addr} ok={str(peer_tcp_ok).lower()} detail={json.dumps(peer_tcp_detail)}",
            flush=True,
        )

    if host_mode == "comms" and not peer_tcp_ok:
        if stop_event is not None and thread is not None and ser is not None:
            stop_event.set()
            thread.join(timeout=2)
            ser.close()
        udp_stop.set()
        udp_thread.join(timeout=2)
        with serial_state.lock:
            peer_id = serial_state.peer_id
            py_calls = serial_state.py_calls
            comms_pass = serial_state.comms_pass
            micropy_init_ok = serial_state.micropy_init_ok
        summary = {
            "status": "fail",
            "failure_reason": "peer_tcp_unreachable_before_host_run",
            "peer_id": peer_id,
            "peer_addr": peer_addr,
            "host_mode": host_mode,
            "peer_tcp_ok": peer_tcp_ok,
            "peer_tcp_detail": peer_tcp_detail,
            "host_listen": host_listen,
            "host_code": None,
            "host_timed_out": False,
            "py_calls": py_calls,
            "comms_pass": comms_pass,
            "micropy_init_ok": micropy_init_ok,
            "host_log": str(artifact_dir / "host.log"),
            "serial_log": str(serial_log),
            "udp_log": str(udp_log),
            "udp_port": udp_port,
        }
        (artifact_dir / "summary.json").write_text(
            json.dumps(summary, indent=2) + "\n", encoding="utf-8"
        )
        print("SUMMARY", json.dumps(summary))
        return 1

    if host_mode == "serial":
        with serial_state.lock:
            host_ip = serial_state.wifi_ip
            host_port = serial_state.serial_port or 4311
        if not host_ip:
            raise SystemExit("serial host mode missing device wifi_ip from UDP markers")
        host_output, host_code, host_timed_out = run_serial_host(
            artifact_dir,
            host_ip,
            host_port,
            serial_state,
            exchanges,
            max(45, exchanges * 15),
        )
    else:
        host_output, host_code, host_timed_out = run_host(
            artifact_dir,
            serial_state.peer_id or "",
            peer_addr,
            host_listen,
            exchanges,
            host_timeout_secs,
        )
        deadline = time.time() + 12
        while time.time() < deadline:
            with serial_state.lock:
                if serial_state.comms_pass:
                    break
            time.sleep(0.1)
    if stop_event is not None and thread is not None and ser is not None:
        stop_event.set()
        thread.join(timeout=2)
        ser.close()
    udp_stop.set()
    udp_thread.join(timeout=2)

    with serial_state.lock:
        peer_id = serial_state.peer_id
        peer_addr = serial_state.peer_addr
        py_calls = serial_state.py_calls
        comms_pass = serial_state.comms_pass
        serial_pass = serial_state.serial_pass
        serial_results = serial_state.serial_results
        micropy_init_ok = serial_state.micropy_init_ok
    summary = {
        "status": (
            "pass"
            if (
                host_mode == "serial"
                and host_code == 0
                and "MKT:HOST_SERIAL:PASS" in host_output
                and serial_pass
                and micropy_init_ok
                and py_calls >= exchanges
                and serial_results >= exchanges
            )
            or (
                host_mode == "comms"
                and host_code == 0
                and "MKT:HOST_MEERKAT:PASS" in host_output
                and comms_pass
                and micropy_init_ok
                and py_calls >= exchanges
            )
            else "fail"
        ),
        "host_mode": host_mode,
        "peer_id": peer_id,
        "peer_addr": peer_addr,
        "peer_tcp_ok": peer_tcp_ok,
        "peer_tcp_detail": peer_tcp_detail,
        "host_listen": host_listen or None,
        "host_code": host_code,
        "host_timed_out": host_timed_out,
        "py_calls": py_calls,
        "serial_pass": serial_pass,
        "serial_results": serial_results,
        "host_log": str(artifact_dir / "host.log"),
        "serial_log": str(serial_log),
        "udp_log": str(udp_log),
        "udp_port": udp_port,
    }
    (artifact_dir / "summary.json").write_text(
        json.dumps(summary, indent=2) + "\n", encoding="utf-8"
    )
    print("SUMMARY", json.dumps(summary))
    return 0 if summary["status"] == "pass" else 1


if __name__ == "__main__":
    raise SystemExit(main())
