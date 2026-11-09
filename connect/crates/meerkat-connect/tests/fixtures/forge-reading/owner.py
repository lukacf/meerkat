"""Actual frozen ConsoleFront + Reading Integration, ephemeral loopback only.

The context has no native runtime or token. Test controls inject transport/owner
failures without replacing the actual configuration, proof or SQLite owners.
"""
import asyncio
import copy
from http.cookies import SimpleCookie
import json
import os
from pathlib import Path
import socket
import sys
from types import SimpleNamespace

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent / "forge-auth"))
sys.path.insert(0, str(HERE))
from console_auth import ConsoleFront
from application import build

PASSWORD = "  fixture läsbar 密码 +&= password  "


async def main():
    root = Path(sys.argv[1])
    os.environ["CONSOLE_USER_ID"] = "fixture-human"
    assert "READING_TELEGRAM_BOT_TOKEN" not in os.environ
    host = {"protocol": 1, "host_id": "forge:" + "a" * 64,
            "application_id": "reading-fixture", "source_digest": "b" * 64}
    context = SimpleNamespace(root=root, deployment={"slug": "reading-fixture"},
                              runtime=None, frontend_host=lambda: copy.deepcopy(host))
    integration = build(context)
    await integration.start()
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        port = sock.getsockname()[1]
    front = ConsoleFront(root, port, "fixture-human", PASSWORD)
    front.frontend_host = context.frontend_host
    front.frontend_private_fields = {"READING_TELEGRAM_BOT_TOKEN"}
    control = {"fail-next-prepare": False, "fail-next-submit": False,
               "wrong-prepare-principal": False}
    requests, commands, canaries = [], [], set()

    async def manage(command):
        commands.append({"operation": command.operation, "principal": command.principal,
                         "host": copy.deepcopy(command.host), "payload": copy.deepcopy(command.payload),
                         "approved": command.approved})
        result = await integration.frontend_management(command)
        if control.get("fail-next-" + command.operation):
            control["fail-next-" + command.operation] = False
            raise RuntimeError("injected after the actual owner result")
        if command.operation == "prepare" and control["wrong-prepare-principal"]:
            control["wrong-prepare-principal"] = False
            result = copy.deepcopy(result)
            result["proposal"]["principal"] = "another-human"
        return result

    front.frontend_management = manage
    original = front.handle

    def result():
        with integration.frontends.connection(readonly=True) as db:
            state = dict(integration.frontends.state(db))
            changes = [dict(row) for row in db.execute("SELECT * FROM reading_frontend_changes ORDER BY rowid")]
        return {"requests": requests, "commands": commands, "state": state,
                "changes": changes, "activation": integration.telegram.observation(),
                "sessions": len(front.sessions), "token_present": "READING_TELEGRAM_BOT_TOKEN" in os.environ}

    async def handle(request):
        response = await original(request)
        requests.append({"method": request.method, "path": request.url.path, "status": response.status_code})
        for header in response.headers.getlist("set-cookie"):
            cookies = SimpleCookie(); cookies.load(header)
            canaries.update(item.value for item in cookies.values() if item.value)
        if request.url.path.endswith("/prepare") and response.status_code == 200:
            canaries.add(json.loads(response.body)["approval"]["proof"])
        return response

    front.handle = handle
    await front.start("http://127.0.0.1:9")
    print(json.dumps({"origin": front.origin}), flush=True)
    try:
        while True:
            line = await asyncio.to_thread(sys.stdin.buffer.readline)
            if not line:
                break
            command = json.loads(line)
            if command == "stop":
                break
            if command == "expire":
                front.sessions.clear()
            elif command == "source-change":
                host["source_digest"] = "c" * 64
            elif command == "wrong-host":
                host["host_id"] = "forge:" + "d" * 64
            elif command == "wrong-principal":
                front.principal = integration.frontends.principal = "another-human"
            elif command == "forget-changes":
                with integration.frontends.connection(readonly=False) as db:
                    db.execute("DELETE FROM reading_frontend_changes")
                    db.commit()
            elif command in control:
                control[command] = True
            elif command != "observe":
                raise ValueError("unknown fixture control")
            print(json.dumps({"ack": command, "observation": result()}), flush=True)
    finally:
        await integration.stop()
        await front.close()
        observed = result()
        observed["server_task_done"] = front.task.done()
        observed["application_task_done"] = integration.telegram.task is None
        (root / "private-canaries.json").write_text(json.dumps(sorted(canaries)))
        (root / "result.json").write_text(json.dumps(observed) + "\n")


asyncio.run(main())
