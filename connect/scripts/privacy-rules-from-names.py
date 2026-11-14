#!/usr/bin/env python3
"""Write privacy scan rules from a private name list.

  python3 -I privacy-rules-from-names.py --names LIST --rules NEW_RULES.json \\
      --exemptions NEW_EXEMPTIONS.json [--review-class CLASS ...] [--paths block|review|off]

The name list is private and never committed: one entry per line, `CLASS<TAB>TOKEN`, with `#` comment
lines. The output is a rules file and an empty exemptions file for release-payload-privacy-scan.py
(schema release-evidence-privacy-scan/rules/1 and exemptions/1), both new files with mode 0600. They
are as private as the list. The rules file names the sha256 of the list it was written from
(`name_list_sha256`); the payload scan repeats it in its report.

Each class becomes one category (lower case, every character outside [a-z0-9_] replaced by `_`), and
each token one rule `<class>-<rank>`, the rank being the token's position among its class's rows in
file order.

How a token matches, always case-insensitively:
  - The boundary rule: where no letter or digit stands before it and no letter after it; a token that
    ends in a digit also needs no digit after it. A plural, a compound or a camel-case form of the
    token is therefore no match.
  - The substring rule, for the classes and the length in SUBSTRING_MATCH (`product` and
    `private_link`, tokens of 6 or more characters): anywhere, inside other words too. These are the
    classes where a missed compound form publishes the name. A shorter token of these classes and
    every token of any other class (a review form of these classes included) keeps the boundary rule.

A class whose name starts with `review-`, and every class named with --review-class, becomes a
category whose name starts with `review_`. The payload scan counts the hits of such a category and
does not refuse them.

Home directory paths (`/home/<name>/`, `/Users/<name>/`) are a built-in category: `home_paths`
(--paths block, the default), `review_home_paths` (--paths review) or absent (--paths off). No class
of the name list may take either name. The payload scan does not pass with rules that have neither
category, or no rule outside the review categories.

The options decide what the rules of a list are: the same list gives other rules with another
option. A release's rules are written with `--paths review` and no `--review-class`, from a list
without a `review-` class, so every rule of a name class refuses; the payload scan's report counts
the review rules outside the path category, and no upload receipt is written unless that is 0.

Standard output holds counts only: the number of rules per category. No token is ever printed.
Exit status: 0 written; 2 a usage or input error (nothing is written).
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import sys
from pathlib import Path

SCHEMA_RULES = "release-evidence-privacy-scan/rules/1"
SCHEMA_EXEMPTIONS = "release-evidence-privacy-scan/exemptions/1"
REVIEW = "review_"
HOME_PATHS = r"/(?:home|Users)/[^/\s\"'<>]+/"
# (the categories, the least token length) whose tokens match as substrings instead of at boundaries.
SUBSTRING_MATCH = (frozenset({"product", "private_link"}), 6)


class Refused(Exception):
    pass


def read_names(path: Path) -> tuple[list[tuple[str, str]], str]:
    """The (class, token) rows of the list and the sha256 of the file they were read from."""
    rows = []
    try:
        raw = path.read_bytes()
        text = raw.decode("utf-8")
    except (OSError, UnicodeError) as error:
        raise Refused(f"the name list cannot be read: {type(error).__name__}") from None
    for number, line in enumerate(text.splitlines(), start=1):
        if not line.strip() or line.lstrip().startswith("#"):
            continue
        name, tab, token = line.partition("\t")
        if not tab or not name.strip() or not token.strip():
            raise Refused(f"line {number} of the name list is not CLASS<TAB>TOKEN")
        rows.append((name.strip(), token.strip()))
    if not rows:
        raise Refused("the name list is empty")
    return rows, hashlib.sha256(raw).hexdigest()


def category_name(name: str, review: bool) -> str:
    base = re.sub(r"[^a-z0-9_]", "_", name.lower())
    if not re.match(r"[a-z]", base):
        base = "c_" + base
    if review and not base.startswith(REVIEW):
        base = REVIEW + base
    return base[:64]


def token_rule(rule_id: str, token: str, category: str) -> dict:
    classes, least = SUBSTRING_MATCH
    rule = {"id": rule_id, "kind": "regex", "ignore_case": True}
    if category in classes and len(token) >= least:
        rule["value"] = re.escape(token)
    else:
        after = r"(?![A-Za-z0-9])" if token[-1].isdigit() else r"(?![A-Za-z])"
        rule["value"] = r"(?<![A-Za-z0-9])" + re.escape(token) + after
    # Every match holds the token, so the scan can skip texts without it. Only for plain ASCII
    # tokens: case folding and case-insensitive matching agree there.
    if token.isascii():
        rule["prefilter"] = [token.casefold()]
    return rule


def build(rows: list[tuple[str, str]], review_classes: set[str], paths: str) -> dict:
    categories: dict[str, dict] = {}
    ranks: dict[str, int] = {}
    seen: set[tuple[str, str]] = set()
    owner: dict[str, str] = {}
    for name, token in rows:
        ranks[name] = ranks.get(name, 0) + 1
        if (name, token.lower()) in seen:
            continue
        seen.add((name, token.lower()))
        category = category_name(name, name.startswith("review-") or name in review_classes)
        if owner.setdefault(category, name) != name:
            raise Refused("two classes of the name list give one category name")
        rule_id = re.sub(r"[^A-Za-z0-9_.-]", "_", name)[:80] + "-" + str(ranks[name])
        if not re.match(r"[A-Za-z0-9]", rule_id):
            rule_id = "c" + rule_id  # the engine's rule ids start with a letter or digit
        categories.setdefault(category, {"rules": []})["rules"].append(token_rule(rule_id, token, category))
    unknown = review_classes - {name for name, _ in rows}
    if unknown:
        raise Refused("--review-class names a class the name list does not have")
    # Both names of the built-in path category are its own, whichever one this call writes: a scan
    # tells that category from a class of names.
    if any(category in categories for category in ("home_paths", REVIEW + "home_paths")):
        raise Refused("a class of the name list uses a name of the built-in path category")
    if paths != "off":
        category = "home_paths" if paths == "block" else REVIEW + "home_paths"
        categories[category] = {"rules": [{"id": "home-paths", "kind": "regex", "value": HOME_PATHS,
                                           "prefilter": ["/home/", "/users/"]}]}
    return {"schema": SCHEMA_RULES, "categories": categories}


def write_new(path: Path, value: dict) -> None:
    descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(descriptor, "w", encoding="utf-8") as stream:
        stream.write(json.dumps(value, indent=1, sort_keys=True) + "\n")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter,
                                     allow_abbrev=False)
    parser.add_argument("--names", type=Path, required=True, help="the private name list")
    parser.add_argument("--rules", type=Path, required=True, help="the rules file to write (a new file)")
    parser.add_argument("--exemptions", type=Path, required=True, help="the exemptions file to write (a new file)")
    parser.add_argument("--review-class", action="append", default=[], metavar="CLASS",
                        help="a class of the list whose hits are counted and not refused")
    parser.add_argument("--paths", choices=("block", "review", "off"), default="block",
                        help="the built-in home directory path category")
    args = parser.parse_args()
    try:
        if args.rules.exists() or args.exemptions.exists() or args.rules == args.exemptions:
            raise Refused("the rules and exemptions must be two new files")
        rows, digest = read_names(args.names)
        rules = build(rows, set(args.review_class), args.paths)
        write_new(args.rules, {**rules, "name_list_sha256": digest})
        write_new(args.exemptions, {"schema": SCHEMA_EXEMPTIONS, "exemptions": []})
    except Refused as error:
        print(f"privacy-rules-from-names: {error}", file=sys.stderr)
        return 2
    except OSError as error:
        print(f"privacy-rules-from-names: {type(error).__name__}", file=sys.stderr)
        return 2
    print(json.dumps({"categories": {name: len(body["rules"]) for name, body in sorted(rules["categories"].items())},
                      "rules": sum(len(body["rules"]) for body in rules["categories"].values())}, sort_keys=True))
    return 0


if __name__ == "__main__":
    sys.exit(main())
