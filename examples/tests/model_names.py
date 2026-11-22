#!/usr/bin/env python3
"""List every example that names a model which is not the newest of its line.

The examples name the newest model of each line the catalog has: a new
developer copies them. This script reads the catalog
(crates/meerkat-models/src/capabilities/*.rs), works out the newest model of
each line, and reads every tracked text file under examples/ and
meta/examples/ for model names.

    python3 examples/tests/model_names.py            # exit 1 and a list when a name is behind
    python3 examples/tests/model_names.py --lines    # the lines and their newest models
    python3 examples/tests/model_names.py docs       # read other trees instead

It needs no build and no network. Run it after every change of the catalog.
It reads the text models of the three providers; image models have a catalog
of their own and are not read.

A name the catalog does not have is listed as unknown and fails the run too:
an example should name a model Meerkat knows. A dated or suffixed form of a
catalog id counts as that id, and a fragment that only starts catalog ids
(`claude-opus-4` in an assertion that no such model is offered) is not a name.

An older model that is named on purpose is kept in one of two ways:
- in the file itself, a line that holds `model-names: keep <model id> (<reason>)`;
- for a file that cannot carry such a line, an entry in KEPT below.

A catalog model that no rule in LINES places ends the run with exit 2: a new
family needs a rule here before anything can be said about it.
"""

from __future__ import annotations

import argparse
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
CATALOG = ROOT / "crates/meerkat-models/src/capabilities"
TREES = ("examples", "meta/examples")

# A line is a product line of one provider: the models in it replace each
# other. Each rule places catalog ids in a line and reads their version. The
# catalog states no lines, so the successions across a change of naming
# (gpt-5.5 to gpt-6-astra, the flash models to gemini-4-hydrogen) are stated
# here, once.
LINES: tuple[tuple[str, str], ...] = (
    (r"claude-(?P<line>fable|opus|sonnet|haiku)-(?P<v>\d+(?:-\d{1,2})?)(?:-\d{8})?", "claude-{line}"),
    (r"gpt-(?P<v>\d+(?:\.\d+)?)-(?P<line>astra|sol|terra|luna)", "gpt-{line}"),
    (r"gpt-(?P<v>5\.6)", "gpt-sol"),
    (r"gpt-(?P<v>5\.[45])(?:-pro)?", "gpt-astra"),
    (r"gpt-(?P<v>5\.4)-mini", "gpt-luna"),
    (r"gpt-(?P<v>\d+(?:\.\d+)?)-codex", "gpt-codex"),
    (r"gpt-live-(?P<v>\d+)", "gpt-live"),
    (r"gpt-live-(?P<v>\d+)-codex", "gpt-live-codex"),
    (r"gpt-realtime-(?P<v>\d+)", "gpt-realtime"),
    (r"gemini-(?P<v>\d+)-(?P<line>carbon|argon|hydrogen|neon)", "gemini-{line}"),
    (r"gemini-(?P<v>\d+(?:\.\d+)?)-flash", "gemini-hydrogen"),
    (r"gemini-(?P<v>\d+(?:\.\d+)?)-pro(?:-preview)?", "gemini-argon"),
    (r"gemini-(?P<v>\d+(?:\.\d+)?)-flash-lite(?:-preview)?", "gemini-flash-lite"),
)

# A model name as it stands in a text: one of the three families, then a digit
# or the live and realtime words.
NAME = re.compile(
    r"(?<![A-Za-z0-9_.-])"
    r"(?:claude-(?:fable|opus|sonnet|haiku)-\d|gpt-(?:\d|live-|realtime-)|gemini-\d)"
    r"[A-Za-z0-9.-]*"
)
KEEP = re.compile(r"model-names: keep (?P<id>[A-Za-z0-9.-]+) \((?P<reason>[^)]+)\)")

# Files that name older models on purpose and cannot say so themselves.
# Path (a directory ends with a slash): reason.
KEPT: dict[str, str] = {
    "examples/AUDIT.md": "a ledger of past reviews; its entries record what was run then",
    "examples/audit/": "a ledger of past reviews; its entries record what was run then",
    "examples/tests/model_names.py": "this script",
}

# Not text, or written by a tool.
SKIPPED_SUFFIXES = {
    ".lock", ".png", ".jpg", ".jpeg", ".gif", ".webp", ".ico", ".svg", ".wasm", ".woff", ".woff2",
    ".ttf", ".mp3", ".wav", ".ogg", ".pdf", ".zip", ".gz", ".mobpack", ".bin",
}
SKIPPED_NAMES = {"package-lock.json", "Cargo.lock", "uv.lock", "pnpm-lock.yaml"}


def version(text: str) -> tuple[int, ...]:
    return tuple(int(part) for part in re.split(r"[.-]", text))


def place(model: str) -> tuple[str, tuple[int, ...]] | None:
    """The line and version of a model id, by the first rule that matches it whole."""
    for pattern, line in LINES:
        match = re.fullmatch(pattern, model)
        if match:
            groups = match.groupdict()
            return line.format(line=groups.get("line", "")), version(groups["v"])
    return None


def catalog_ids() -> list[str]:
    ids: list[str] = []
    for path in sorted(CATALOG.glob("*.rs")):
        ids += re.findall(r"ModelCapabilities \{\s*id: \"([^\"]+)\"", path.read_text(encoding="utf-8"))
    if not ids:
        sys.exit(f"no model id found under {CATALOG.relative_to(ROOT)}: the catalog has moved")
    return ids


def newest_by_line(ids: list[str]) -> tuple[dict[str, str], dict[str, str]]:
    """Returns (line of each catalog id, newest id of each line)."""
    line_of: dict[str, str] = {}
    best: dict[str, tuple[tuple[int, ...], str]] = {}
    unplaced = []
    for model in ids:
        placed = place(model)
        if placed is None:
            unplaced.append(model)
            continue
        line, number = placed
        line_of[model] = line
        # Of two ids with one version (a dated id beside its alias), the shorter is the name to use.
        if line not in best or (number, -len(model)) > (best[line][0], -len(best[line][1])):
            best[line] = (number, model)
    if unplaced:
        print("The catalog has models that no rule in LINES places:", ", ".join(unplaced), file=sys.stderr)
        print("Add a rule to examples/tests/model_names.py, then run it again.", file=sys.stderr)
        sys.exit(2)
    return line_of, {line: model for line, (_, model) in best.items()}


def tracked_files(trees: list[str]) -> list[str]:
    listed = subprocess.run(
        ["git", "-C", str(ROOT), "ls-files", "-z", "--", *trees],
        check=True, capture_output=True,
    ).stdout.decode("utf-8")
    return [name for name in listed.split("\0") if name]


def kept_reason(path: str) -> str | None:
    for prefix, reason in KEPT.items():
        if path == prefix or (prefix.endswith("/") and path.startswith(prefix)):
            return reason
    return None


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("trees", nargs="*", default=list(TREES), help="trees to read (default: examples meta/examples)")
    parser.add_argument("--lines", action="store_true", help="print each line and its newest model, then exit")
    parser.add_argument("--kept", action="store_true", help="also list the older names that are kept on purpose")
    args = parser.parse_args()

    ids = catalog_ids()
    line_of, newest = newest_by_line(ids)
    if args.lines:
        for line in sorted(newest):
            older = sorted(model for model in ids if line_of[model] == line and model != newest[line])
            print(f"{line}: {newest[line]}" + (f"   (older: {', '.join(older)})" if older else ""))
        return 0

    behind: list[str] = []
    unknown: list[str] = []
    kept: list[str] = []
    unused_keeps: list[str] = []
    files = 0
    for path in tracked_files(args.trees):
        file = ROOT / path
        if file.suffix in SKIPPED_SUFFIXES or file.name in SKIPPED_NAMES or not file.is_file():
            continue
        try:
            text = file.read_text(encoding="utf-8")
        except UnicodeDecodeError:
            continue
        files += 1
        whole_file = kept_reason(path)
        keeps = {match["id"]: match["reason"] for match in KEEP.finditer(text)}
        used = set()
        for number, line_text in enumerate(text.splitlines(), start=1):
            if KEEP.search(line_text):
                continue
            for match in NAME.finditer(line_text):
                model = match.group(0).rstrip(".-")
                where = f"{path}:{number}: {model}"
                if model not in line_of:
                    # A catalog id with a date after it, or the leading parts
                    # of one (a family named in prose). A misspelled id is
                    # neither: it is reported as not in the catalog.
                    extends = any(re.fullmatch(re.escape(known) + r"-\d{8}", model) for known in ids)
                    fragment = any(known.startswith((model + "-", model + ".")) for known in ids)
                    if extends or fragment or whole_file is not None:
                        continue
                    if model in keeps:
                        used.add(model)
                        kept.append(f"{where} (kept: {keeps[model]})")
                    else:
                        unknown.append(where)
                    continue
                current = newest[line_of[model]]
                if model == current:
                    continue
                if whole_file is not None:
                    kept.append(f"{where} (file kept: {whole_file})")
                elif model in keeps:
                    used.add(model)
                    kept.append(f"{where} (kept: {keeps[model]})")
                else:
                    behind.append(f"{where} -> {current}")
        unused_keeps += [f"{path}: keeps {model}, which it no longer needs to" for model in keeps if model not in used]

    if args.kept:
        for entry in kept:
            print("kept  ", entry)
    for entry in unknown:
        print("unknown", entry, "(not in the catalog)")
    for entry in unused_keeps:
        print("stale ", entry)
    for entry in behind:
        print("behind", entry)
    print(
        f"{files} files read under {' and '.join(args.trees)}: {len(behind)} behind the newest of their line, "
        f"{len(unknown)} not in the catalog, {len(kept)} kept on purpose, {len(unused_keeps)} stale keep lines"
    )
    return 1 if behind or unknown or unused_keeps else 0


if __name__ == "__main__":
    sys.exit(main())
