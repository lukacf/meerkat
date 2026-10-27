"""Local TLS wire fixture. All certificates and credentials are inert test data."""
from datetime import datetime, timedelta, timezone
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import ipaddress
import json
from pathlib import Path
import ssl
import threading
import time

from cryptography import x509
from cryptography.hazmat.primitives import hashes, serialization
from cryptography.hazmat.primitives.asymmetric import rsa
from cryptography.x509.oid import NameOID

INPUT = {"type": "object", "properties": {"n": {"type": "integer", "minimum": 1}},
         "required": ["n"], "additionalProperties": False}
OUTPUT = {"type": "object", "properties": {"value": {"type": "integer"}},
          "required": ["value"], "additionalProperties": False}


def certificates(directory, hostname="localhost"):
    directory = Path(directory)
    key = rsa.generate_private_key(public_exponent=65537, key_size=2048)
    name = x509.Name([x509.NameAttribute(NameOID.COMMON_NAME, "Toolkit inert local CA")])
    now = datetime.now(timezone.utc)
    ca = (x509.CertificateBuilder().subject_name(name).issuer_name(name)
          .public_key(key.public_key()).serial_number(x509.random_serial_number())
          .not_valid_before(now - timedelta(days=1)).not_valid_after(now + timedelta(days=1))
          .add_extension(x509.BasicConstraints(ca=True, path_length=0), critical=True)
          .sign(key, hashes.SHA256()))
    leaf_key = rsa.generate_private_key(public_exponent=65537, key_size=2048)
    leaf_name = x509.Name([x509.NameAttribute(NameOID.COMMON_NAME, hostname)])
    alternatives = [x509.DNSName(hostname)]
    if hostname == "localhost":
        alternatives.append(x509.IPAddress(ipaddress.ip_address("127.0.0.1")))
    leaf = (x509.CertificateBuilder().subject_name(leaf_name).issuer_name(name)
            .public_key(leaf_key.public_key()).serial_number(x509.random_serial_number())
            .not_valid_before(now - timedelta(days=1)).not_valid_after(now + timedelta(days=1))
            .add_extension(x509.SubjectAlternativeName(alternatives), critical=False)
            .add_extension(x509.BasicConstraints(ca=False, path_length=None), critical=True)
            .sign(key, hashes.SHA256()))
    paths = [directory / name for name in ("ca.pem", "server.pem", "server.key")]
    paths[0].write_bytes(ca.public_bytes(serialization.Encoding.PEM))
    paths[1].write_bytes(leaf.public_bytes(serialization.Encoding.PEM))
    paths[2].write_bytes(leaf_key.private_bytes(serialization.Encoding.PEM,
                         serialization.PrivateFormat.PKCS8, serialization.NoEncryption()))
    return paths


class Peer:
    def __init__(self, directory, mode="normal", hostname="localhost"):
        self.ca, self.cert, self.key = certificates(directory, hostname)
        self.mode, self.events, self.pending = mode, [], None
        self.call_seen = threading.Event()
        self.release = threading.Event()
        self.listings = 0
        self.tag = "runtime-" + Path(directory).name
        peer = self

        class Handler(BaseHTTPRequestHandler):
            protocol_version = "HTTP/1.1"

            def log_message(self, *args):
                pass

            def record(self, body=None):
                peer.events.append({"http_method": self.command, "path": self.path,
                                    "headers": dict(self.headers), "message": body})

            def reply(self, status, body=b"", content_type="application/json", extra=()):
                self.send_response(status)
                self.send_header("Content-Type", content_type)
                self.send_header("Content-Length", str(len(body)))
                for key, value in extra:
                    self.send_header(key, value)
                self.end_headers()
                try:
                    self.wfile.write(body)
                    self.wfile.flush()
                except (OSError, ssl.SSLError):
                    pass

            def result(self, identity, result, extra=()):
                result.setdefault("resultType", "complete")
                if "tools" in result or "supportedVersions" in result:
                    result.update(cacheScope="private", ttlMs=0)
                self.reply(200, json.dumps({"jsonrpc": "2.0", "id": identity, "result": result}).encode(), extra=extra)

            def do_POST(self):
                body = json.loads(self.rfile.read(int(self.headers.get("Content-Length", "0"))))
                self.record(body)
                method, identity = body.get("method"), body.get("id")
                if peer.mode == "unauthorized":
                    self.reply(401, b"PRIVATE_BODY", extra=(("WWW-Authenticate", 'Bearer resource_metadata="https://PRIVATE_AUTH"'),))
                elif identity is None:
                    self.reply(202)
                elif method == "server/discover":
                    self.result(identity, {"supportedVersions": ["2026-07-28"], "capabilities": {"tools": {}}})
                elif method == "initialize":
                    self.result(identity, {"protocolVersion": "2025-11-25", "capabilities": {"tools": {}},
                                           "serverInfo": {"name": "inert-tls-peer", "version": "1"}},
                                extra=(("Mcp-Session-Id", "PRIVATE_SESSION"), ("Set-Cookie", "PRIVATE_COOKIE=present; Path=/; Secure")))
                elif method == "tools/list":
                    peer.listings += 1
                    tool = {"name": "increment", "title": "Observed " + peer.tag,
                            "description": peer.tag, "inputSchema": INPUT, "outputSchema": OUTPUT,
                            "annotations": {"readOnlyHint": True}, "_meta": {"inert": peer.tag}}
                    if peer.mode == "param_header":
                        tool["inputSchema"] = dict(INPUT, properties={"n": {"type": "integer", "minimum": 1, "x-mcp-header": "Number"}})
                    if peer.mode == "nulls":
                        tool.update(title=None, description=None, outputSchema=None, annotations=None, _meta=None)
                    if peer.mode == "absent":
                        for name in ("title", "description", "outputSchema", "annotations", "_meta"):
                            tool.pop(name)
                    if peer.mode == "changed" or peer.mode == "later_changed" and peer.listings > 1:
                        tool["inputSchema"] = {"type": "object"}
                    result = {"tools": [] if peer.mode == "empty" else [tool]}
                    if peer.mode in ("pages", "duplicate", "cycle"):
                        cursor = body.get("params", {}).get("cursor")
                        if cursor and peer.mode != "duplicate":
                            tool["name"] = "page" + str(peer.listings)
                        if not cursor or peer.mode == "cycle":
                            result["nextCursor"] = "next"
                    if peer.mode == "overflow":
                        tool["description"] = "x" * 6000
                    if peer.mode == "nonfinal":
                        result["resultType"] = "input_required"
                    self.result(identity, result)
                elif method == "tools/call":
                    peer.call_seen.set()
                    if peer.mode in ("redirect", "cross_redirect"):
                        location = "/PRIVATE_REDIRECT" if peer.mode == "redirect" else "https://127.0.0.1:1/PRIVATE_REDIRECT"
                        self.reply(307, extra=(("Location", location),))
                    elif peer.mode == "timeout":
                        peer.release.wait(10)
                        self.result(identity, {"content": []})
                    elif peer.mode == "dropped":
                        self.close_connection = True
                        self.connection.shutdown(2)
                    elif peer.mode == "input":
                        self.result(identity, {"resultType": "input_required", "requestState": "PRIVATE_STATE"})
                    elif peer.mode in ("resume", "endless_resume"):
                        peer.pending = (identity, body["params"]["arguments"]["n"] + 1)
                        self.reply(200, b"event: message\nid: PRIVATE_EVENT\nretry: 10\ndata:\n\n", "text/event-stream")
                    elif peer.mode == "rpc_error":
                        self.reply(200, json.dumps({"jsonrpc": "2.0", "id": identity,
                            "error": {"code": -32000, "message": "PRIVATE_EXCEPTION", "data": "PRIVATE_ERROR_DATA"}}).encode())
                    else:
                        value = body["params"]["arguments"]["n"] + 1
                        self.result(identity, {"content": [{"type": "text", "text": str(value)}],
                                               "structuredContent": {"value": value}})
                else:
                    self.reply(500)

            def do_GET(self):
                self.record()
                if peer.pending and self.headers.get("Last-Event-ID"):
                    if peer.mode == "endless_resume":
                        self.reply(200, b"event: message\nid: PRIVATE_EVENT\nretry: 10\ndata:\n\n", "text/event-stream")
                    else:
                        identity, value = peer.pending
                        body = json.dumps({"jsonrpc": "2.0", "id": identity, "result": {"resultType": "complete",
                            "content": [{"type": "text", "text": str(value)}], "structuredContent": {"value": value}}})
                        self.reply(200, ("event: message\ndata: " + body + "\n\n").encode(), "text/event-stream")
                else:
                    self.reply(405)

            def do_DELETE(self):
                self.record()
                self.reply(500 if peer.mode == "delete_error" else 204)

        self.server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.server.daemon_threads = True
        context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
        context.load_cert_chain(self.cert, self.key)
        self.server.socket = context.wrap_socket(self.server.socket, server_side=True)
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()
        self.url = "https://127.0.0.1:" + str(self.server.server_port) + "/mcp?bound=PRIVATE_QUERY"

    def close(self):
        self.release.set()
        self.server.shutdown()
        self.server.server_close()
        self.thread.join(timeout=5)
        if self.thread.is_alive():
            raise RuntimeError("local fixture did not close")
