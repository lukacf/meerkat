"""Loopback acceptance using the frozen real Forge auth owner, no application/native runtime."""
import asyncio
import json
from http.cookies import SimpleCookie
from pathlib import Path
import socket
import sys

sys.path.insert(0, str(Path(__file__).resolve().parent))
from console_auth import ConsoleFront

PASSWORD = "  fixture läsbar 密码 +&= password  "

async def main():
    root = Path(sys.argv[1])
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        port = sock.getsockname()[1]
    front = ConsoleFront(root, port, "fixture-human", PASSWORD)
    records = []
    canaries = set()
    original = front.handle

    async def handle(request):
        response = await original(request)
        records.append({"method": request.method, "path": request.url.path,
                        "status": response.status_code})
        for header in response.headers.getlist("set-cookie"):
            cookies = SimpleCookie(); cookies.load(header)
            canaries.update(item.value for item in cookies.values() if item.value)
        return response

    front.handle = handle
    # No frontend callback/identity and no native upstream is started.
    await front.start("http://127.0.0.1:9")
    print(json.dumps({"origin": front.origin}), flush=True)
    try:
        await asyncio.to_thread(sys.stdin.buffer.read, 1)
    finally:
        await front.close()
        (root / "private-canaries.json").write_text(json.dumps(sorted(canaries)))
        (root / "result.json").write_text(json.dumps({"requests": records,
            "sessions_after_close": len(front.sessions),
            "server_task_done": front.task.done(),
            "producer": front.frontend_management is not None,
            "host_identity": front.frontend_host is not None}) + "\n")

asyncio.run(main())
