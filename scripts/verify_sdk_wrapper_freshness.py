#!/usr/bin/env python3
"""Verify SDK source coverage for canonical app-facing RPC catalog methods."""

from __future__ import annotations

import pathlib
import re
import sys


def main() -> int:
    if len(sys.argv) != 2:
        print("Usage: verify_sdk_wrapper_freshness.py REPO_ROOT", file=sys.stderr)
        return 2

    root = pathlib.Path(sys.argv[1])

    catalog_path = root / "meerkat-contracts" / "src" / "rpc_catalog.rs"
    ts_root = root / "sdks" / "typescript" / "src"
    py_root = root / "sdks" / "python" / "meerkat"

    catalog_text = catalog_path.read_text(encoding="utf-8")

    catalog_methods = sorted(
        {
            m
            for m in re.findall(
                r'RpcMethodDescriptor::(?:basic|typed|params_only|result_only)\(\s*"([^"]+)"',
                catalog_text,
            )
        }
    )

    internal_exclusions = {
        "initialize",
        "session/stream_open",
        "session/stream_close",
        "mob/stream_open",
        "mob/stream_close",
    }

    required_methods = [m for m in catalog_methods if m not in internal_exclusions]

    ts_blob = "\n".join(
        p.read_text(encoding="utf-8", errors="ignore") for p in ts_root.rglob("*.ts")
    )
    py_blob = "\n".join(
        p.read_text(encoding="utf-8", errors="ignore") for p in py_root.rglob("*.py")
    )

    missing_ts = [m for m in required_methods if m not in ts_blob]
    missing_py = [m for m in required_methods if m not in py_blob]

    if missing_ts:
        print("TypeScript SDK appears to be missing wrappers/usages for methods:")
        for method in missing_ts:
            print(f"  - {method}")

    if missing_py:
        print("Python SDK appears to be missing wrappers/usages for methods:")
        for method in missing_py:
            print(f"  - {method}")

    if missing_ts or missing_py:
        return 1

    print(
        "SDK wrapper freshness OK: all required catalog methods are represented "
        "in TS + Python source."
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
