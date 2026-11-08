"""Bounded strip-debug producer step for a Connect binary (gateway or companion CLI).

One run of the pinned objcopy with --strip-debug into a separate output, with its own START and RESULT records
and a retained, byte-pinned output. The original is only read: it is pinned before and after the run and never
opened for writing.

  python3 -I derive-strip-debug.py --binary-kind {toolkit_gateway,meerkat-connect} --original ABS \\
      --original-sha256 HEX --original-size N --output-dir ABS [--timeout-seconds N]

--output-dir must not exist, its parent must be canonical, and it must lie outside the original's directory.
It receives:
  <binary kind>        the derived ELF, mode 0555: the retained candidate bytes
  START.json           written and fsynced before the tool starts
  stdout, stderr       the tool's own output
  RESULT.json          exit code, class, seconds, and the input, output and tool pins re-read after the run
  DERIVED-BINARY.json  the assembler's derived-binary record, written only when the run succeeded
Exit 0 means the derived output is retained. Exit 4 means refused or failed; RESULT.json says how, once the tool
has started.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import time
from datetime import datetime, timezone

TOOL = Path("/usr/bin/x86_64-linux-gnu-objcopy")
TOOL_SHA = "9276e918e3c26d3aa7ef9dd2232cd8ac692f03d3c1f270bfa5144a2b5492b713"
TOOL_VERSION = "GNU objcopy (GNU Binutils for Ubuntu) 2.42"
KINDS = {"toolkit_gateway": "meerkat_connect_derived_gateway", "meerkat-connect": "meerkat_connect_derived_cli"}
ENVIRONMENT = {"LC_ALL": "C", "PATH": "/usr/bin:/bin"}
TERM_GRACE = 15.0


class Refused(Exception):
    pass


def require(condition, message):
    if not condition:
        raise Refused(message)


def digest(path):
    value = hashlib.sha256()
    with open(path, "rb") as stream:
        while block := stream.read(16 * 1024 * 1024):
            value.update(block)
    return value.hexdigest()


def identity(path):
    return {"path": str(path), "sha256": digest(path), "size": path.stat().st_size}


def now():
    return datetime.now(timezone.utc).isoformat(timespec="microseconds").replace("+00:00", "Z")


def write_record(path, value):
    raw = (json.dumps(value, indent=2, sort_keys=True) + "\n").encode()
    with open(path, "xb") as stream:
        stream.write(raw)
        stream.flush()
        os.fsync(stream.fileno())
    return {"path": str(path), "sha256": hashlib.sha256(raw).hexdigest()}


def stream_pin(path):
    return {"path": str(path), "sha256": digest(path), "bytes": path.stat().st_size}


def tool_version():
    run = subprocess.run([str(TOOL), "--version"], env=ENVIRONMENT, cwd="/", stdin=subprocess.DEVNULL,
                         capture_output=True, timeout=30, check=False)
    lines = run.stdout.decode("utf-8", "replace").splitlines()
    return run.returncode, lines[0] if lines else ""


def derive(args):
    original, output_dir = args.original, args.output_dir
    require(original.is_absolute() and not original.is_symlink() and original.is_file()
            and original.resolve(strict=True) == original, "canonical absolute regular original required")
    require(output_dir.is_absolute() and not output_dir.exists() and not output_dir.is_symlink()
            and output_dir.parent.resolve(strict=True) == output_dir.parent
            and not output_dir.is_relative_to(original.parent),
            "fresh output directory with a canonical parent, outside the original's directory, required")
    require(1 <= args.timeout_seconds <= 3600, "timeout must be 1-3600 seconds")
    before = identity(original)
    require(before["sha256"] == args.original_sha256 and before["size"] == args.original_size,
            "original differs from its pin")
    require(digest(TOOL) == TOOL_SHA, "strip tool differs from its pin")
    code, version = tool_version()
    require(code == 0 and version == TOOL_VERSION, "strip tool version differs from its pin")
    output_dir.mkdir(mode=0o755)
    derived = output_dir / args.binary_kind
    argv = [str(TOOL), "--strip-debug", str(original), str(derived)]
    tool = {"path": str(TOOL), "sha256": TOOL_SHA, "version": TOOL_VERSION}
    producer = Path(__file__).resolve()
    start = write_record(output_dir / "START.json", {
        "schema_version": 1, "kind": "meerkat_connect_strip_debug_start", "binary_kind": args.binary_kind,
        "argv": argv, "tool": tool, "input": before, "output": {"path": str(derived)},
        "timeout_seconds": args.timeout_seconds, "term_grace_seconds": TERM_GRACE, "environment": ENVIRONMENT,
        "cwd": "/", "started_at": now(), "producer": {"path": str(producer), "sha256": digest(producer)}})
    began = time.monotonic()
    outcome = "ok"
    with open(output_dir / "stdout", "xb") as stdout, open(output_dir / "stderr", "xb") as stderr:
        process = subprocess.Popen(argv, env=ENVIRONMENT, cwd="/", stdin=subprocess.DEVNULL, stdout=stdout,
                                   stderr=stderr, start_new_session=True)
        try:
            code = process.wait(timeout=args.timeout_seconds)
        except subprocess.TimeoutExpired:
            outcome = "timeout"
            os.killpg(process.pid, signal.SIGTERM)
            try:
                code = process.wait(timeout=TERM_GRACE)
            except subprocess.TimeoutExpired:
                os.killpg(process.pid, signal.SIGKILL)
                code = process.wait()
    seconds = round(time.monotonic() - began, 3)
    if outcome == "ok" and code != 0:
        outcome = "failed"
    after = identity(original)
    output = None
    if derived.is_file() and not derived.is_symlink():
        if outcome == "ok":
            derived.chmod(0o555)
        output = identity(derived)
    if outcome == "ok" and (after != before or output is None or not 0 < output["size"] <= before["size"]):
        outcome = "output_rejected"
    tool_after = {"path": str(TOOL), "sha256": digest(TOOL)}
    if outcome == "ok" and tool_after["sha256"] != TOOL_SHA:
        outcome = "tool_changed"
    result = write_record(output_dir / "RESULT.json", {
        "schema_version": 1, "kind": "meerkat_connect_strip_debug_result", "start_sha256": start["sha256"],
        "exit_code": code, "class": outcome, "seconds": seconds, "finished_at": now(), "input": after,
        "output": output, "output_mode": oct(derived.stat().st_mode & 0o777) if output else None,
        "tool": tool_after, "stdout": stream_pin(output_dir / "stdout"),
        "stderr": stream_pin(output_dir / "stderr")})
    if outcome != "ok":
        return 4, {"status": "failed", "class": outcome, "exit_code": code, "start": start, "result": result}
    record = write_record(output_dir / "DERIVED-BINARY.json", {
        "schema_version": 1, "kind": KINDS[args.binary_kind], "derivation": "strip_debug",
        "binary_kind": args.binary_kind, "original": before, "tool": tool, "argv": argv, "derived": output,
        "transformation": {"start": start, "result": result}})
    return 0, {"status": "derived_output_retained", "derived": output, "record": record, "start": start,
               "result": result, "seconds": seconds}


def main():
    parser = argparse.ArgumentParser(description=__doc__, allow_abbrev=False,
                                     formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--binary-kind", choices=tuple(KINDS), required=True)
    parser.add_argument("--original", type=Path, required=True)
    parser.add_argument("--original-sha256", required=True)
    parser.add_argument("--original-size", type=int, required=True)
    parser.add_argument("--output-dir", type=Path, required=True)
    parser.add_argument("--timeout-seconds", type=int, default=900)
    args = parser.parse_args()
    try:
        code, report = derive(args)
    except Refused as error:
        code, report = 4, {"status": "refused", "problem": str(error)}
    print(json.dumps(report, sort_keys=True))
    return code


if __name__ == "__main__":
    sys.exit(main())
