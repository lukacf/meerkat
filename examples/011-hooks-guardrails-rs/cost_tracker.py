"""Observe one hook invocation on stdin; append bounded usage metadata."""

import json
from pathlib import Path
import sys


def main():
    payload = sys.stdin.buffer.read(65_537)
    if len(payload) > 65_536:
        raise ValueError("hook payload exceeds 64 KiB")
    invocation = json.loads(payload)
    usage = (invocation.get("llm_response") or {}).get("usage")
    row = {
        "session_id": invocation["session_id"],
        "point": invocation["point"],
        "turn_number": invocation.get("turn_number"),
        "usage": usage,
    }
    line = json.dumps(row, separators=(",", ":")) + "\n"
    if len(line.encode("utf-8")) > 8192:
        raise ValueError("usage record exceeds 8 KiB")
    log = Path(sys.argv[1])
    log.parent.mkdir(parents=True, exist_ok=True)
    with log.open("a", encoding="utf-8") as output:
        output.write(line)
    print("{}")


if __name__ == "__main__":
    main()
