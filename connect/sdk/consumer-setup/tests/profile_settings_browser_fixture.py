"""UI-only profile selection fixture. No installer, credentials or provider calls."""
import argparse
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
from pathlib import Path
import signal
import threading
import time


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--assets", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=False)
    operations = [{"id": "read_fixture", "effect": "read"}]
    profiles = ["first-profile", "second-profile", "delayed-profile", "retry-profile"]
    state = {"families": [{"id": "agno", "status": "prepared", "profiles": [
        {"id": profile, "operations": operations} for profile in profiles
    ]}], "job": None, "scope": "local_installation_only", "operation_in_progress": False}
    inspection = threading.Lock()
    counts = {}

    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *_args):
            pass

        def reply(self, code, data, content_type="application/json"):
            self.send_response(code)
            self.send_header("Content-Type", content_type)
            self.send_header("Content-Length", str(len(data)))
            self.send_header("Cache-Control", "no-store")
            self.end_headers()
            self.wfile.write(data)

        def do_GET(self):
            name = "index.html" if self.path == "/" else self.path.removeprefix("/")
            if name not in {"index.html", "app.js", "form.js", "style.css"}:
                return self.reply(404, b"{}")
            content_type = {"html": "text/html", "js": "text/javascript", "css": "text/css"}[name.rsplit(".", 1)[1]]
            self.reply(200, (args.assets / name).read_bytes(), content_type)

        def do_POST(self):
            payload = json.loads(self.rfile.read(int(self.headers.get("Content-Length", "0"))))
            with (args.output / "requests.jsonl").open("a") as log:
                log.write(json.dumps({"path": self.path, "payload": payload}) + "\n")
            if self.headers.get("X-Toolkit-Session") != "fixture-profile-settings":
                return self.reply(403, b"{}")
            if self.path == "/api/state" and payload == {}:
                return self.reply(200, json.dumps(state).encode())
            if self.path != "/api/help" or set(payload) != {"family", "profile"} or payload["family"] != "agno" or payload["profile"] not in profiles:
                return self.reply(409, b'{"error":"fixture_refusal"}')
            if not inspection.acquire(blocking=False):
                return self.reply(409, b'{"error":"operation_in_progress"}')
            try:
                profile = payload["profile"]
                counts[profile] = counts.get(profile, 0) + 1
                if profile == "delayed-profile":
                    time.sleep(3)
                if profile == "retry-profile" and counts[profile] == 1:
                    return self.reply(503, b'{"error":"fixture_unavailable"}')
                help_value = {"family": "agno", "profile": profile, "operations": operations,
                    "selectable_operations": True, "fields": [{"name": "resource", "kind": "string", "required": False, "credential_reference": False}],
                    "prerequisites": [], "unresolved_details": [], "scope": "configuration_only",
                    "guide_text": "Synthetic configuration guidance. No software or account is configured."}
                self.reply(200, json.dumps(help_value).encode())
            finally:
                inspection.release()

    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)

    def stop(*_args):
        threading.Thread(target=server.shutdown, daemon=True).start()

    signal.signal(signal.SIGTERM, stop)
    timer = threading.Timer(600, stop)
    timer.daemon = True
    timer.start()
    (args.output / "server.json").write_text(json.dumps({"port": server.server_port,
        "url": f"http://127.0.0.1:{server.server_port}/#session=fixture-profile-settings"}))
    try:
        server.serve_forever()
    finally:
        server.server_close()
        timer.cancel()


if __name__ == "__main__":
    main()
