"""Local transport/command fixtures for the actual smoke and readiness functions."""
import json
import os
from pathlib import Path
import socket
import subprocess
import tempfile
import threading
import time
import unittest

ROOT = Path(__file__).resolve().parents[1]


class Scripts(unittest.TestCase):
    def rpc_response(self, response):
        with socket.socket() as listener:
            listener.bind(("127.0.0.1", 0))
            listener.listen()

            def serve():
                conn, _ = listener.accept()
                with conn, conn.makefile("rwb") as stream:
                    request = json.loads(stream.readline())
                    stream.write(json.dumps({
                        "jsonrpc": "2.0", "id": request["id"], **response,
                    }).encode() + b"\n")
                    stream.flush()

            thread = threading.Thread(target=serve)
            thread.start()
            result = subprocess.run(
                ["bash", "-c", "source scripts/docker-smoke.sh; check_hive_rpc_session"],
                cwd=ROOT, env={**os.environ, "HIVE_RPC_PORT": str(listener.getsockname()[1])},
                capture_output=True, text=True, timeout=10,
            )
            thread.join(timeout=2)
        self.assertFalse(thread.is_alive())
        return result

    def test_rpc_error_preserves_server_diagnostic(self):
        result = self.rpc_response({
            "error": {"code": -32001, "message": "synthetic server failure"},
        })
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("synthetic server failure", result.stderr)
        self.assertIn("-32001", result.stderr)
        self.assertNotIn("NameError", result.stderr)

    def test_rpc_success_preserves_session_id(self):
        result = self.rpc_response({"result": {"sessions": [{"session_id": "synthetic-session"}]}})
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("synthetic-session", result.stdout)

    def test_restart_rejects_historical_readiness(self):
        with tempfile.TemporaryDirectory(dir=ROOT) as scratch:
            docker = Path(scratch) / "docker"
            docker.write_text("""#!/usr/bin/env bash
case "$*" in
  *"restart target-b") exit 0 ;;
  *"ps -q target-b") echo owned-fixture ;;
  "inspect --format "* ) echo 2026-09-22T12:00:00Z ;;
  *"logs --no-color --since 2026-09-22T12:00:00Z target-b")
    if [[ "${FRESH_LOGS:-0}" == 1 ]]; then
      echo "session ready; added hive as trusted peer"
    elif [[ "${FRESH_LOGS:-0}" == delayed ]]; then
      if [[ -f "$READINESS_CURSOR" ]]; then
        echo "session ready; added hive as trusted peer"
      else
        touch "$READINESS_CURSOR"
      fi
    fi ;;
  *"logs "* ) echo "session ready; added hive as trusted peer" ;;
  *) exit 2 ;;
esac
""")
            docker.chmod(0o755)
            for fresh in ["0", "1", "delayed"]:
                started = time.monotonic()
                result = subprocess.run(
                    ["bash", "-c", "source scripts/tmux-architecture-suite.sh; restart_target_b"],
                    cwd=ROOT,
                    env={**os.environ, "PATH": f"{scratch}:{os.environ['PATH']}",
                         "TUX_ARCH_RESTART_WAIT_SECONDS": "3" if fresh == "delayed" else "0",
                         "READINESS_CURSOR": str(Path(scratch) / "cursor"), "FRESH_LOGS": fresh},
                    capture_output=True, text=True, timeout=10,
                )
                self.assertEqual(result.returncode == 0, fresh != "0", result.stderr)
                if fresh == "delayed":
                    self.assertGreaterEqual(time.monotonic() - started, 1.8)


if __name__ == "__main__":
    unittest.main()
