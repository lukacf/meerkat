#!/usr/bin/env python3
"""Write the upload receipts of a staging directory from its two full scan reports. Runs where the scans ran.

  python3 -I release-upload-receipt-write.py --privacy-report REPORT --credential-report REPORT \\
      --version N.N.N --target TARGET --receipt NEW_RECEIPT.json \\
      [--target TARGET --receipt NEW_RECEIPT.json ...] [acceptance bindings]

--target and --receipt are given in pairs, in order: one receipt per target. One pair of reports,
each scan having read every staged file in one run, gives the receipt of every target of that
staging directory in one call: a release has one for its platform target and one for the target
`release`. Each receipt holds exactly the rows whose file names are its target's by the committed
name rules (release/upload-receipt-pins.json of the checkout this tool runs from, read as
release-upload-receipt-check.py reads it). --version is the release's version: the file names of
every target are held to it, so a platform's file named for another release gets no row.

The two full reports (release-payload-privacy-scan.py and release-payload-credential-scan.py) stay
where they were written. They hold more than an upload check needs: category names, rule and
finding counts. What leaves that place is a receipt, and only it: the upload receipt check
(release-upload-receipt-check.py), accept-publication, publish-accepted and the release workflow
read the receipt and never a full report.

The receipt (schema release-upload-receipt/4) holds exactly what the check reads:
  {"schema": ..., "target": TARGET,
   "payload": [{"name", "bytes", "sha256"}],   the target's files, as both reports name them
   "privacy": {"status", "release", "exit_code", "generated_at", "tool_commit", "tool_sha256",
               "rules_sha256", "name_list_sha256", "non_published_list_sha256",
               "coverage_exceptions_sha256", "blocking_rules": true,
               "name_rules_all_blocking": true, "no_exemptions": true,
               "accepted_only_with_proof": true, "accepted_proof_tools": {TOOL: entries},
               "report_sha256"},
   "credential": {"status", "release", "generated_at", "tool_commit", "tool_sha256",
                  "detector_pinned_sha256", "detector_current_sha256",
                  "detector_pinned_default_rules_sha256", "detector_current_default_rules_sha256",
                  "nothing_refused": true,
                  "nothing_pending": true, "nothing_unread_unaccepted": true, "report_sha256"}}
The two default rules digests are of the rules file each build read with, as the credential
report states them (detector_builds, default_rules_sha256): the check holds them to its pins.
Digests, times, the status word, seven facts as true or false, and the names of the tools whose
proofs the used acceptance entries carry, each with the number of entries it proved (so that
whoever reads a receipt before an upload sees which tools vouched, and for how much). No category
name, no count of rules, findings or exemptions, no label, and nothing else of what an acceptance
file holds. report_sha256 is the sha256 of the full report the leg was written from; the private-text
report holds a random value of the scan (nonce), so that digest confirms no guess of the report's
other fields, and a report without it gives no receipt. The receipts of one
call differ in target and payload only.

Receipts are written only from two passing release reports, all of the call or none. Nothing is
written when a report
  - is missing, is not a JSON object of its tool's report schema, or lacks a field named above;
  - does not say exactly "PASS", is not a release report (release true), gives an exit_code
    other than 0, or is named PROOF-ONLY (its file name, its name or label, or a payload name);
  - states a tool that is not the scan's own file: tool_sha256 must be the sha256 of that scan's
    file (beside this tool) as the report's tool_commit holds it in this tool's checkout. A
    report of a commit the checkout does not hold gives no receipt. The check does not repeat
    this: it holds a receipt, not the scans' files;
  - is a credential report that names an identifier class proof (identifier_classes) and does not
    state, as list_sha256, the sha256 of release/payload-credential-identifier-classes.json as the
    report's tool_commit holds it in this tool's checkout: a finding is resolved by the commit's
    class list and no other;
  - says PASS and shows otherwise: the private-text report's reasons, hits.refused and
    errors.refused must be empty, its exemptions.blocking and wrapper_failures 0, and its
    engine.exit_code the one that follows from the hits and errors it shows. The status word
    is not taken for it;
  - shows no blocking rule, a refused or pending finding, or an unread file that is not accepted;
  - is a credential report that does not count, for each of its two builds, the files the build
    was handed and left unread for lack of proof of read (read_proof, unread), or counts one: no
    acceptance covers such a file for an upload;
  - does not show that every rule outside the path category is blocking: a review class of names
    refuses nothing, so the only review rule may be the path category's (the converter's
    `--paths review`), and the report must count none outside it (rules.review_outside_paths 0);
  - counts an exemption (exemptions.count is not 0): an exemption switches a match off, and the
    exemptions file is held by no pin, so a release scan runs with none;
  - shows a used acceptance entry that names no proof tool (accepted.used_unproved is not 0), or
    tool counts that do not stand with what was accepted, or a tool name that is not of the one
    form (a lower-case letter or a digit, then lower-case letters, digits and single hyphens, 64
    characters at most; the names go into the receipt as they are): a finding is accepted by a
    tool's proof, not by hand. The writer does not verify a proof;
  - names a file the other report does not name, or the same file name with other bytes: both
    scans must have read every file;
and when
  - a file is a file name of none of the targets given, or a target given gets no file: no
    receipt vouches for a part of the files;
  - --version is missing;
  - the pins file is not what this checkout's commit holds, is not of its schema, or has no name
    rule for a target given.

Acceptance bindings, for a caller that holds the acceptance inputs (--privacy-accepted-...,
--credential-accepted-..., --credential-pending-..., each as -sha256 HEX|none or -file FILE; a file
that does not exist means none): the report must have been made with exactly that input.

Whether the receipt passes is the check's to say: the pins, the release commit, the age and the
files as they are at the upload are compared there, from the checkout the check runs in.

Output: every reason, one per line, then "UPLOAD-RECEIPT REFUSED"; or one line per receipt, in
the order given,
  UPLOAD-RECEIPT WRITTEN target=<target> files=<n> receipt_sha256=<sha256>
Exit status: 0 written, 1 refused (nothing written), 2 usage error (nothing written). Standard
library only; git is run to read the commit's pins file.
"""
from __future__ import annotations

import argparse
import hashlib
import importlib.util
import json
import os
import re
import sys
from pathlib import Path

SCHEMA = "release-upload-receipt/4"
CHECK_NAME = "release-upload-receipt-check.py"
# The file of each scan, beside this tool: what a report's tool_sha256 is the digest of.
SCAN_FILES = {"privacy report": "release-payload-privacy-scan.py", "credential report": "release-payload-credential-scan.py"}
# The committed class list of the credential scan, in release/ of the checkout: a report that names an
# identifier class proof states the list's digest.
CLASSES_NAME = "payload-credential-identifier-classes.json"
PRIVACY_SCHEMA = re.compile(r"release-payload-privacy-scan/report/[0-9]+\Z")
CREDENTIAL_SCHEMA = re.compile(r"release-payload-credential-scan/report/[0-9]+\Z")
HEX64 = re.compile(r"[0-9a-f]{64}\Z")
COMMIT = re.compile(r"[0-9a-f]{40}([0-9a-f]{24})?\Z")
STAMP = re.compile(r"[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}Z\Z")
TARGET = re.compile(r"[a-z0-9][a-z0-9_-]{0,63}\Z")
VERSION = re.compile(r"[0-9]+\.[0-9]+\.[0-9]+\Z")
NONCE = re.compile(r"[0-9a-f]{32}\Z")
# The one form of a proof tool's name, as the private-text scan and the receipt check hold it: a
# lower-case letter or a digit, then lower-case letters, digits and single hyphens, 64 at most.
PROOF_TOOL = re.compile(r"(?!.*--)[a-z0-9][a-z0-9-]{0,63}\Z")
PROOF_ONLY = "PROOF-ONLY"
BINDINGS = ("privacy-accepted", "credential-accepted", "credential-pending")


class Usage(Exception):
    pass


def sha256_file(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def count(value) -> bool:
    return isinstance(value, int) and not isinstance(value, bool) and value >= 0


def digest(value) -> bool:
    return isinstance(value, str) and HEX64.match(value) is not None


def part_of(report: dict, field: str, part: str):
    """report[field][part], or None when the report has no such object."""
    return report[field].get(part) if isinstance(report.get(field), dict) else None


def tally(*groups) -> int | None:
    """The sum of the counts of {name: count} objects; None when one of them is no such object."""
    if not all(isinstance(value, dict) and all(count(number) for number in value.values()) for value in groups):
        return None
    return sum(number for value in groups for number in value.values())


def binding(args, name: str) -> str | None:
    """An acceptance binding: a digest, or the digest of the given file. "none" for no file."""
    given, path = getattr(args, name.replace("-", "_") + "_sha256"), getattr(args, name.replace("-", "_") + "_file")
    if given is not None and path is not None:
        raise Usage(f"--{name}-sha256 and --{name}-file are two forms of one binding; give one")
    if given is None and path is None:
        return None
    if path is not None:
        return sha256_file(path) if path.is_file() else "none"
    if HEX64.match(given) or given == "none":
        return given
    raise Usage(f"--{name}-sha256 must be 64 lower-case hex digits or none")


def summary(value, problems: list, label: str, field: str, bound: str | None) -> None:
    """A {count, sha256} field of a report; when bound, made with exactly that input."""
    if not isinstance(value, dict) or not count(value.get("count")) or "sha256" not in value \
            or not (value["sha256"] is None or digest(value["sha256"])):
        problems.append(f"{label}: {field} does not give a count and a sha256")
    elif bound is not None and (value["sha256"] or "none") != bound:
        problems.append(f"{label}: {field} was made with another input than the bound one")


def read(label: str, path: Path, schema, problems: list) -> tuple[dict, dict, dict] | None:
    """One full report: (the report, the fields both legs share, {name: (bytes, sha256)} of its payload).
    None when it cannot be read at all."""
    if PROOF_ONLY in path.name:
        problems.append(f"{label}: the report file is a {PROOF_ONLY} report")
    if not path.is_file():
        problems.append(f"{label}: the report is missing")
        return None
    try:
        raw = path.read_bytes()
        report = json.loads(raw)
    except (OSError, ValueError):
        problems.append(f"{label}: the report is not readable JSON")
        return None
    if not isinstance(report, dict):
        problems.append(f"{label}: the report is not a JSON object")
        return None
    if not isinstance(report.get("schema"), str) or not schema.match(report["schema"]):
        problems.append(f"{label}: schema is not this tool's report schema")
    if report.get("status") != "PASS" or not isinstance(report.get("status"), str):
        problems.append(f"{label}: status is not PASS")
    if report.get("release") is not True:
        problems.append(f"{label}: release is not true (a run that is not a release run gets no receipt)")
    if any(isinstance(report.get(key), str) and report[key].startswith(PROOF_ONLY) for key in ("name", "label")):
        problems.append(f"{label}: a {PROOF_ONLY} run is not a release report")
    if "exit_code" in report and (report["exit_code"] != 0 or isinstance(report["exit_code"], bool)):
        problems.append(f"{label}: exit_code is not 0")
    if not isinstance(report.get("generated_at"), str) or not STAMP.match(report["generated_at"]):
        problems.append(f"{label}: generated_at is missing or not a UTC time")
    if not isinstance(report.get("tool_commit"), str) or not COMMIT.match(report["tool_commit"]):
        problems.append(f"{label}: tool_commit is not a full commit id")
    if not digest(report.get("tool_sha256")):
        problems.append(f"{label}: tool_sha256 is not a sha256")
    payload, rows = report.get("payload"), {}
    if not isinstance(payload, list) or not payload or not all(
            isinstance(row, dict) and isinstance(row.get("name"), str) and count(row.get("bytes"))
            and digest(row.get("sha256")) for row in payload) or len({row["name"] for row in payload}) != len(payload):
        problems.append(f"{label}: payload does not list files, each once, with name, bytes and sha256")
    else:
        rows = {row["name"]: (row["bytes"], row["sha256"]) for row in payload}
        if any(name.startswith(PROOF_ONLY) for name in rows):
            problems.append(f"{label}: payload names a {PROOF_ONLY} file")
    shared = {"status": report.get("status"), "release": report.get("release"), "generated_at": report.get("generated_at"),
              "tool_commit": report.get("tool_commit"), "tool_sha256": report.get("tool_sha256"),
              "report_sha256": hashlib.sha256(raw).hexdigest()}
    return report, shared, rows


def checkout_of_this_tool(problems: list):
    """(the upload receipt check's module, its view of the checkout this tool runs from: the commit and
    the files that commit holds); (None, None) with the reason in problems. The writer reads the
    checkout as the check reads it, through the check's own git call."""
    path = Path(__file__).resolve().with_name(CHECK_NAME)
    sys.dont_write_bytecode = True
    try:
        spec = importlib.util.spec_from_file_location("release_upload_receipt_check", path)
        check = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(check)
    except (OSError, ImportError, AttributeError, SyntaxError):
        problems.append(f"pins: {CHECK_NAME} is not beside this tool: the checkout is read as the check reads it")
        return None, None
    try:
        return check, check.Checkout()
    except check.Refused as error:
        problems.extend(error.reasons)
        return None, None


def name_rules(check, checkout, problems: list):
    """The name rules of the pins file this checkout's commit holds; None with the reason in problems."""
    try:
        sections, _ = checkout.sections()
    except check.Refused as error:
        problems.extend(error.reasons)
        return None
    return check.Names({target: section["names"] for target, section in sections.items()})


def scan_file_is_its_commits(label: str, report: dict, checkout, problems: list) -> None:
    """The scan's own bytes: the sha256 a report states for its tool must be that of the scan's file, beside
    this tool, as the commit the report names holds it in this checkout. The scans ran where this tool runs."""
    commit, stated = report.get("tool_commit"), report.get("tool_sha256")
    if checkout is None or not isinstance(commit, str) or not COMMIT.match(commit) or not digest(stated):
        return  # each of these has its own reason
    held = checkout.blob_sha256(commit, Path(__file__).resolve().with_name(SCAN_FILES[label]))
    if held is None:
        problems.append(f"{label}: tool_commit is not a commit of this checkout that holds {SCAN_FILES[label]}: "
                        "the scan's own file cannot be compared")
    elif held != stated:
        problems.append(f"{label}: tool_sha256 is not {SCAN_FILES[label]} as the report's tool_commit holds it")


def class_list_is_its_commits(label: str, report: dict, checkout, problems: list) -> None:
    """The class list of a credential report: a report that names an identifier class proof states the
    sha256 of the class list that was in force, and that must be release/CLASSES_NAME as the commit
    the report names holds it in this checkout. A report without that part resolved nothing by class."""
    if "identifier_classes" not in report:
        return
    classes, commit = report["identifier_classes"], report.get("tool_commit")
    stated = classes.get("list_sha256") if isinstance(classes, dict) else None
    if not digest(stated):
        problems.append(f"{label}: identifier_classes.list_sha256 is not a sha256: release/{CLASSES_NAME} cannot be compared")
    elif checkout is not None and isinstance(commit, str) and COMMIT.match(commit) \
            and checkout.blob_sha256(commit, Path(__file__).resolve().parent.parent / "release" / CLASSES_NAME) != stated:
        problems.append(f"{label}: identifier_classes.list_sha256 is not release/{CLASSES_NAME} as the report's tool_commit holds it")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter,
                                     allow_abbrev=False)
    parser.add_argument("--target", action="append", required=True, help="a target of the files; once per receipt")
    parser.add_argument("--receipt", type=Path, action="append", required=True,
                        help="the receipt to write for the --target at the same place: a new file")
    parser.add_argument("--version", help="the release's version, N.N.N: the file names of every target are held to it")
    parser.add_argument("--privacy-report", type=Path, required=True)
    parser.add_argument("--credential-report", type=Path, required=True)
    for name in BINDINGS:
        parser.add_argument(f"--{name}-sha256")
        parser.add_argument(f"--{name}-file", type=Path)
    args = parser.parse_args()
    try:
        if len(args.target) != len(args.receipt):
            raise Usage("--target and --receipt are given in pairs: one receipt for each target")
        if not all(TARGET.match(target) for target in args.target):
            raise Usage("--target is not a target name")
        if len(set(args.target)) != len(args.target):
            raise Usage("a --target is given twice")
        if len({path.absolute() for path in args.receipt}) != len(args.receipt):
            raise Usage("a --receipt is given twice")
        if any(PROOF_ONLY in path.name for path in args.receipt):
            raise Usage(f"a receipt is never named {PROOF_ONLY}")
        if args.version is not None and not VERSION.match(args.version):
            raise Usage("--version must be the release's version in the form N.N.N")
        if any(os.path.lexists(path) for path in args.receipt):
            raise Usage("--receipt must be a new file")
        bound = {name: binding(args, name) for name in BINDINGS}
    except Usage as error:
        print(f"release-upload-receipt-write: usage error: {error}", file=sys.stderr)
        return 2
    problems: list[str] = []
    scans = {}
    check, checkout = checkout_of_this_tool(problems)

    label = "privacy report"
    privacy = read(label, args.privacy_report, PRIVACY_SCHEMA, problems)
    if privacy is not None:
        report, shared, privacy_rows = privacy
        scan_file_is_its_commits(label, report, checkout, problems)
        if "exit_code" not in report:
            problems.append(f"{label}: exit_code is missing")
        if not isinstance(report.get("nonce"), str) or not NONCE.match(report["nonce"]):
            problems.append(f"{label}: nonce is not the scan's random value (without it report_sha256 confirms a guess of the report)")
        rules = report.get("rules")
        blocking = isinstance(rules, dict) and count(rules.get("blocking")) and count(rules.get("total")) \
            and 0 < rules["blocking"] <= rules["total"]
        if not blocking:
            problems.append(f"{label}: rules does not show at least one blocking rule")
        # Every rule of a name class is blocking: the only review rule a release's rules may hold is
        # the path category's one. The report counts the review rules outside that category.
        if not blocking or not count(rules.get("review_outside_paths")) or rules["review_outside_paths"] != 0 \
                or rules["total"] - rules["blocking"] > 1:
            problems.append(f"{label}: rules does not show that every rule outside the path category is blocking "
                            "(review_outside_paths 0, and at most the one review rule of the path category)")
        if not isinstance(rules, dict) or not digest(rules.get("sha256")):
            problems.append(f"{label}: rules.sha256 is not a sha256")
        for field in ("name_list_sha256", "non_published_list_sha256"):
            if not digest(report.get(field)):
                problems.append(f"{label}: {field} is null or not a sha256")
        for field in ("exemptions", "accepted", "coverage_exceptions"):
            summary(report.get(field), problems, label, field, bound["privacy-accepted"] if field == "accepted" else None)
        # An exemption switches a match off, and no pin holds the exemptions file: a release scan has none.
        exemptions = report.get("exemptions")
        if not isinstance(exemptions, dict) or not count(exemptions.get("count")) or exemptions["count"] != 0:
            problems.append(f"{label}: exemptions.count is not 0 (a release scan runs with no exemption)")
        # What the report shows is compared with what it says: PASS beside a reason, a refused hit or
        # error, an exemption in a blocking category or a failure of the walk is not a passing report.
        if not isinstance(report.get("reasons"), list) or report["reasons"]:
            problems.append(f"{label}: reasons is not an empty list (a passing report has no reason for a refusal)")
        for group in ("hits", "errors"):
            refused = report[group].get("refused") if isinstance(report.get(group), dict) else None
            if not isinstance(refused, dict) or refused:
                problems.append(f"{label}: {group}.refused is not empty")
        if not isinstance(exemptions, dict) or not count(exemptions.get("blocking")) or exemptions["blocking"] != 0:
            problems.append(f"{label}: exemptions.blocking is not 0")
        if not count(report.get("wrapper_failures")) or report["wrapper_failures"] != 0:
            problems.append(f"{label}: wrapper_failures is not 0")
        # The engine's exit code: 1 when it saw a hit, 2 when it saw an error. A passing report shows
        # where each went, so the code that belongs to it follows from what it shows.
        engine = report.get("engine")
        code = engine.get("exit_code") if isinstance(engine, dict) else None
        hits = tally(part_of(report, "hits", "accepted"), report.get("review_hits"))
        errors = tally(part_of(report, "errors", "covered"), part_of(report, "errors", "accepted"), part_of(report, "coverage_exceptions", "used"))
        if hits is None or errors is None or isinstance(code, bool) or not isinstance(code, int) \
                or code != (1 if hits else 0) | (2 if errors else 0):
            problems.append(f"{label}: engine.exit_code does not stand with what the report shows (1 for a hit: accepted "
                            "or review hits; 2 for an error: covered, accepted or excepted errors)")
        # A finding is accepted only by a tool's proof: every acceptance entry the scan used names the
        # tool that proved it. The report counts the used entries that name none, and the rest by tool.
        # The names go into the receipt as they are, so each is of the one form of a tool's name.
        accepted = report.get("accepted") if isinstance(report.get("accepted"), dict) else {}
        tools, taken = accepted.get("used_by_tool"), tally(part_of(report, "hits", "accepted"), part_of(report, "errors", "accepted"))
        named = isinstance(tools, dict) and all(isinstance(name, str) and PROOF_TOOL.match(name) and count(number) and number >= 1
                                                for name, number in tools.items())
        if not count(accepted.get("used_unproved")) or accepted["used_unproved"] != 0 or not named or taken is None \
                or not count(accepted.get("count")) or sum(tools.values()) > accepted["count"] or bool(tools) != bool(taken):
            problems.append(f"{label}: accepted does not show that every used acceptance entry names its proof tool "
                            "(used_unproved 0, and used_by_tool {tool: entries} standing with what was accepted, each "
                            "tool's name lower-case letters, digits and single hyphens, 64 at most)")
        exceptions = report.get("coverage_exceptions")
        if not isinstance(exceptions, dict) or not digest(exceptions.get("sha256")):
            problems.append(f"{label}: coverage_exceptions.sha256 is null or not a sha256 (the scan ran without the committed file)")
        if not problems:
            scans["privacy"] = {
                **shared, "exit_code": 0, "rules_sha256": rules["sha256"], "name_list_sha256": report["name_list_sha256"],
                "non_published_list_sha256": report["non_published_list_sha256"],
                "coverage_exceptions_sha256": exceptions["sha256"], "blocking_rules": True, "name_rules_all_blocking": True,
                "no_exemptions": True, "accepted_only_with_proof": True, "accepted_proof_tools": dict(tools)}

    label = "credential report"
    before = len(problems)
    credential = read(label, args.credential_report, CREDENTIAL_SCHEMA, problems)
    if credential is not None:
        report, shared, credential_rows = credential
        scan_file_is_its_commits(label, report, checkout, problems)
        class_list_is_its_commits(label, report, checkout, problems)
        builds, seen, rules = report.get("detector_builds"), {}, {}
        if isinstance(builds, list) and len(builds) == 2 and all(isinstance(row, dict) for row in builds):
            seen = {row.get("role"): row.get("sha256") for row in builds}
            rules = {row.get("role"): row.get("default_rules_sha256") for row in builds}
        if set(seen) != {"pinned", "current"} or not all(digest(value) for value in seen.values()):
            problems.append(f"{label}: detector_builds does not name a pinned and a current build with their sha256")
        # The rules file each build read with. The check holds it to a pin: a build's digest does not fix it.
        elif not all(digest(value) for value in rules.values()):
            problems.append(f"{label}: detector_builds does not state the sha256 of the default rules file each build "
                            "read with (default_rules_sha256)")
        for group, field in (("findings", "refused"), ("findings", "pending_owner"), ("unread", "unaccepted")):
            value = report.get(group, {}).get(field) if isinstance(report.get(group), dict) else None
            if not count(value) or value != 0:
                problems.append(f"{label}: {group}.{field} is not 0")
        # Proof of read. The report counts, for each of its two builds, the files the build was handed
        # and left unread for lack of proof of read. For an upload each count is 0, whatever an
        # acceptance says: no entry covers such a file.
        proofs, versions = report.get("read_proof"), {row.get("version") for row in builds} if seen else set()
        if not isinstance(proofs, dict) or not versions or set(proofs) != versions or not all(
                isinstance(proof, dict) and count(proof.get("unread")) for proof in proofs.values()):
            problems.append(f"{label}: read_proof does not count, for each of the two builds, the files left unread for lack of proof of read")
        elif any(proof["unread"] != 0 for proof in proofs.values()):
            problems.append(f"{label}: read_proof counts a file that a build left unread for lack of proof of read (unread is not 0)")
        summary(report.get("accepted"), problems, label, "accepted", bound["credential-accepted"])
        summary(report.get("pending"), problems, label, "pending", bound["credential-pending"])
        if len(problems) == before:
            scans["credential"] = {
                **shared, "detector_pinned_sha256": seen["pinned"], "detector_current_sha256": seen["current"],
                "detector_pinned_default_rules_sha256": rules["pinned"],
                "detector_current_default_rules_sha256": rules["current"],
                "nothing_refused": True, "nothing_pending": True, "nothing_unread_unaccepted": True}

    # The rows: every file both reports name with the same bytes, each under the one target given
    # whose committed name rule its name meets. A file that only one report names, and a file that is
    # a name of none of the targets given, leave no receipt at all.
    payloads: dict[str, list] = {target: [] for target in args.target}
    if privacy is not None and credential is not None and privacy_rows and credential_rows:
        common = sorted(set(privacy_rows) & set(credential_rows))
        for name in sorted(set(privacy_rows) ^ set(credential_rows)):
            problems.append(f"payload: only the {'privacy' if name in privacy_rows else 'credential'} report names {name}: "
                            "both scans must have read every file")
        for name in common:
            if privacy_rows[name] != credential_rows[name]:
                problems.append(f"payload: the two reports name other bytes for {name}")
        if not common:
            problems.append("payload: the two reports name no file in common")
        names = name_rules(check, checkout, problems) if checkout is not None else None
        if names is not None:
            unknown = [target for target in args.target if target not in names.rules]
            if unknown:
                problems.append(f"pins: release/{check.PINS_FILE.name} has no name rule for the target {', '.join(unknown)}")
            elif args.version is None:
                problems.append("payload: --version is required: the file names of every target are held to the release's version")
            else:
                for name in common:
                    homes = [target for target in args.target
                             if names.homes(name, target, args.version) == {target}]
                    if len(homes) != 1:
                        problems.append(f"payload: {name} is not a file name of exactly one of the targets given "
                                        f"({', '.join(args.target)}) for the version {args.version}; no receipt is written "
                                        "for a part of the files")
                    else:
                        payloads[homes[0]].append({"name": name, "bytes": privacy_rows[name][0], "sha256": privacy_rows[name][1]})
                for target in args.target:
                    if not payloads[target]:
                        problems.append(f"payload: no file is a file of the target {target}")

    if problems or set(scans) != {"privacy", "credential"} or not all(payloads.values()):
        for problem in problems:
            print(problem)
        print(f"UPLOAD-RECEIPT REFUSED: {len(problems)} reason(s); no receipt was written")
        return 1
    # All or nothing: every receipt of the call, or none of them.
    written, lines = [], []
    for target, path in zip(args.target, args.receipt):
        raw = (json.dumps({"schema": SCHEMA, "target": target, "payload": payloads[target], **scans},
                          indent=2, sort_keys=True) + "\n").encode()
        try:
            descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o644)
        except OSError as error:
            for earlier in written:
                earlier.unlink()
            print(f"release-upload-receipt-write: usage error: --receipt must be a new file: {error.strerror}", file=sys.stderr)
            return 2
        with os.fdopen(descriptor, "wb") as stream:
            stream.write(raw)
        written.append(path)
        lines.append("UPLOAD-RECEIPT WRITTEN target=%s files=%d receipt_sha256=%s" % (
            target, len(payloads[target]), hashlib.sha256(raw).hexdigest()))
    print("\n".join(lines))
    return 0


if __name__ == "__main__":
    sys.exit(main())
