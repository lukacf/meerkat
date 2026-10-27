#!/usr/bin/env python3
"""Fail when CI's unit test filter leaves out a lib or bin test.

The `test` job in .github/workflows/ci.yml is the only automatic run of the
workspace's lib and bin tests: the pre-push hook's unit gate is manual, and
the other jobs run integration tests or a named feature subset. A test its
filter drops therefore runs nowhere until someone runs it by hand. The job
lists its tests with the run's own filter (`nextest list --message-format
json`) and passes the listing here. Every listed test must match the
filter, apart from the ones the source marks #[ignore], and every listed
binary must be listed rather than skipped.

To leave a lib or bin test out of the job, first make another automatic job
run it, then change this check to name that job.

Usage: verify-unit-test-filter.py LISTING_JSON
"""

import json
import sys
from pathlib import Path


def excluded_tests(listing: dict) -> list[str]:
    """The listed tests the filter drops, as `binary-id test (reason)`."""
    excluded = []
    for binary_id, suite in sorted(listing["rust-suites"].items()):
        status = suite.get("status", "listed")
        if status != "listed":
            excluded.append(f"{binary_id} (the whole binary: {status})")
            continue
        for name, case in sorted(suite["testcases"].items()):
            match = case["filter-match"]
            if match["status"] != "matches" and match.get("reason") != "ignored":
                excluded.append(f"{binary_id} {name} ({match.get('reason', match['status'])})")
    return excluded


def main(argv: list[str]) -> int:
    if len(argv) != 2:
        print(f"usage: {Path(argv[0]).name} LISTING_JSON", file=sys.stderr)
        return 2
    listing = json.loads(Path(argv[1]).read_text(encoding="utf-8"))
    suites = listing.get("rust-suites") or {}
    cases = [case for suite in suites.values() for case in suite.get("testcases", {}).values()]
    if not cases:
        print(f"{argv[1]} lists no tests, so the listing failed", file=sys.stderr)
        return 1
    excluded = excluded_tests(listing)
    for test in excluded:
        print(f"left out of the unit test job: {test}")
    if excluded:
        print(f"the filter drops {len(excluded)} lib or bin tests, which then run in no CI job",
              file=sys.stderr)
        return 1
    ignored = sum(1 for case in cases if case["filter-match"]["status"] != "matches")
    print(f"{len(cases) - ignored} lib and bin tests run in the unit test job; "
          f"{ignored} more are #[ignore]d in source")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
