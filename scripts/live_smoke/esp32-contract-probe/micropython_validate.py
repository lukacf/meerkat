#!/usr/bin/env python3

from __future__ import annotations

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
        self.ready = False
        self.comms_pass = False
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


def reset_and_monitor(port: str, serial_log: pathlib.Path) -> tuple[serial.Serial, SerialState, threading.Event, threading.Thread]:
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

            state = SerialState()
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
                    if state.ready and state.peer_id and state.peer_addr:
                        print(
                            f"MKT:VALIDATOR:DEVICE_READY peer_id={state.peer_id} peer_addr={state.peer_addr}",
                            flush=True,
                        )
                        return ser, state, stop_event, thread
                time.sleep(0.1)

            stop_event.set()
            thread.join(timeout=2)
            ser.close()
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
        os.environ.get("ESP32_HOST_LISTEN", "192.168.0.197:4220"),
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
            elif proc.poll() is not None:
                return "".join(lines), proc.wait(), timed_out

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


def main() -> int:
    port = os.environ.get("ESP32_PORT", "/dev/cu.usbmodem101")
    exchanges = int(os.environ.get("ESP32_EXCHANGES", "10"))
    host_timeout_secs = int(os.environ.get("ESP32_HOST_TIMEOUT_SECS", "180"))
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

    ser, serial_state, stop_event, thread = reset_and_monitor(port, serial_log)
    host_output, host_code, host_timed_out = run_host(
        artifact_dir,
        serial_state.peer_id or "",
        serial_state.peer_addr or "",
        exchanges,
        host_timeout_secs,
    )
    deadline = time.time() + 12
    while time.time() < deadline:
        with serial_state.lock:
            if serial_state.comms_pass:
                break
        time.sleep(0.1)
    stop_event.set()
    thread.join(timeout=2)
    ser.close()

    with serial_state.lock:
        peer_id = serial_state.peer_id
        peer_addr = serial_state.peer_addr
        py_calls = serial_state.py_calls
        comms_pass = serial_state.comms_pass
        micropy_init_ok = serial_state.micropy_init_ok
    summary = {
        "status": (
            "pass"
            if host_code == 0
            and "MKT:HOST_MEERKAT:PASS" in host_output
            and comms_pass
            and micropy_init_ok
            and py_calls >= exchanges
            else "fail"
        ),
        "peer_id": peer_id,
        "peer_addr": peer_addr,
        "host_code": host_code,
        "host_timed_out": host_timed_out,
        "py_calls": py_calls,
        "host_log": str(artifact_dir / "host.log"),
        "serial_log": str(serial_log),
    }
    (artifact_dir / "summary.json").write_text(
        json.dumps(summary, indent=2) + "\n", encoding="utf-8"
    )
    print("SUMMARY", json.dumps(summary))
    return 0 if summary["status"] == "pass" else 1


if __name__ == "__main__":
    raise SystemExit(main())
