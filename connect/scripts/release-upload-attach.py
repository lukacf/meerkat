#!/usr/bin/env python3
"""Attach the files of one staging directory to an existing Connect release. Fails closed.

  python3 -I release-upload-attach.py --staging DIR --tag connect-vX.Y.Z \\
      --receipt TARGET=RECEIPT [--receipt TARGET=RECEIPT ...] [--release-commit COMMIT]

This is the one way a file reaches a Connect release, beside connect-candidate.py
publish-accepted, which adds its own three candidate files. Every file this tool attaches is a
row of an upload receipt that release-upload-receipt-check.py passed in this run. One release is
one run: the staging directory holds every file of the release, and each target that has files
there has one receipt (a platform target such as linux-x86_64, and `release` for the files
that name no platform). A run needs the receipt of `release` and of at least one platform
target: neither part of a release goes up alone.

Steps, in this order. Each one refuses on any failure, and nothing is uploaded unless every
step before the uploads passed.

 a. The reviewed commit. This tool runs from a git checkout, and this file, the receipt check
    and connect-candidate.py are byte for byte what the checkout's commit holds; the code that
    runs is those committed bytes. That commit must be on the default branch of the release
    repository. The tool asks the repository (the fixed public address of connect-candidate.py's
    PUBLICATION_REPO, never the checkout's own remote) for the name of its default branch and
    the commit at its tip, with a git call that reads no configuration at all, so no rewrite
    rule can send the question elsewhere. It then fetches that branch, completes a shallow
    history, and requires the checkout's commit to be an ancestor of the tip it was told. The
    fetch only brings objects: it writes no ref and no FETCH_HEAD and moves no remote-tracking
    ref. An unknown branch, a failed fetch, a history that cannot be completed, a tip the fetch
    did not bring and a commit that is not an ancestor all refuse; nothing skips this step. The
    git calls ignore replace refs, a grafts file, a commit-graph file and every GIT_ variable of
    the caller. These reads of the repository are the only network access before the check has
    passed.
 b. The staging directory. Its entries must be exactly the rows of the receipts: an entry that
    is no row refuses, a row without an entry refuses, a name under two receipts refuses, and
    every entry must be a regular file (a link, a directory or a special file refuses).
 c. The check. Each file and each receipt is first copied into a private custody directory
    (made beside the staging directory, removed at the end) and each copy must be its row's
    size and sha256. The receipt check then runs once per receipt, on the custody copies, with
    exactly that receipt's rows. The version it holds the file names of every receipt to is the
    tag's. The release commit is this checkout's commit unless --release-commit names another.
    The pins the check judged by must be those of the commit step a passed: a pass that names
    another commit as the one its pins were read from (a checkout that moved in between) refuses.
 d. The uploads. The release of the tag must exist, be published (a draft refuses) and its tag
    must select the release commit. Before the first write every asset already present must be
    a candidate file of publish-accepted or a row; a present asset that is a row must read
    back with exactly the row's size and sha256, and then counts as attached without a new
    upload (a rerun after a partial run completes it; other bytes under a row's name refuse).
    Each missing row is uploaded from its custody copy, read back from the release, and its
    size and sha256 compared with the row. The read-back is an authenticated read of the asset.
    The upload and the read-back have a longer time bound than a read or a listing. An upload
    that is cut off can leave an asset under the row's name that is not the row's bytes: every
    rerun then refuses, and this tool never deletes it; an owner of the release removes it.
 e. The end. The release's assets are listed again: an asset that is neither a row of a
    receipt this run checked nor a candidate file refuses with a non-zero exit and is named.
    Nothing is deleted. So a later run whose receipts do not name an asset of an earlier run,
    for example one for another platform alone, refuses on that asset before it uploads
    anything: a later run must bring every earlier file again, unchanged, with its receipts.
 f. Add-only. This tool never creates, edits, drafts, publishes, deletes or replaces a release
    or an asset. Its only write is the upload of a new asset to the existing release.

Every access to GitHub goes through the class GitHub below: reads, the one upload request
(connect-candidate.py's release_asset_upload) and the read-back of an asset.

Output: one line per row, "<attached|present> <target> <name> <sha256> <bytes>", then
  RELEASE-ATTACH PASS tag=<tag> files=<n> attached=<k> present=<p> release_commit=<commit>
      tool_commit=<commit> default_branch=<name>
or every reason for the refusal, one per line, the rows done so far, and
  RELEASE-ATTACH REFUSED tag=<tag> attached=<k> present=<p>: <n> reason(s)
It prints counts, names and digests of rows and the check's reasons. It never reads a scan
report. Exit status: 0 pass, 1 refused, 2 usage error. Standard library only.
"""
from __future__ import annotations

import argparse
import hashlib
import os
import re
import shutil
import stat
import subprocess
import sys
import tempfile
import types
from pathlib import Path
from typing import NamedTuple

TOOL = Path(__file__).resolve()
SCRIPTS = TOOL.parent
CHECK_FILE = SCRIPTS / "release-upload-receipt-check.py"
CANDIDATE_FILE = SCRIPTS / "connect-candidate.py"
TAG = re.compile(r"connect-v([0-9]+\.[0-9]+\.[0-9]+)\Z")
TARGET = re.compile(r"[a-z0-9][a-z0-9_-]{0,63}\Z")
NAME = re.compile(r"[A-Za-z0-9][A-Za-z0-9._-]{0,254}\Z")
COMMIT = re.compile(r"[0-9a-f]{40}\Z")
BRANCH = re.compile(r"[A-Za-z0-9][A-Za-z0-9._/-]*\Z")
DEFAULT_BRANCH = re.compile(rb"^ref: refs/heads/([^\t\n]+)\tHEAD$", re.M)
DEFAULT_TIP = re.compile(rb"^([0-9a-f]{40})\tHEAD$", re.M)
RELEASE_TARGET = "release"
REPOSITORY = "https://github.com/%s.git"
NETWORK_SECONDS = 900
CHECK_ENTRY = ("check", "receipt_rows", "Refused", "Usage")


class Usage(Exception):
    pass


class Refused(Exception):
    def __init__(self, reasons, done=()):
        super().__init__("; ".join(reasons))
        self.reasons, self.done = list(reasons), list(done)


class Row(NamedTuple):
    target: str
    name: str
    size: int
    sha256: str

    def binding(self):
        return {"name": self.name, "size": self.size, "sha256": self.sha256}


def git_environment():
    """The environment of every git call of this tool: no GIT_ variable of the caller, no prompt, and
    no grafts file (a grafts file would state other parents than the commits themselves hold)."""
    environment = {key: value for key, value in os.environ.items() if not key.startswith("GIT_")}
    environment["GIT_TERMINAL_PROMPT"] = "0"
    environment["GIT_GRAFT_FILE"] = os.devnull
    return environment


def git(*arguments, timeout=120):
    """What a git command prints, run in this tool's directory; None when it fails. No GIT_ variable
    of the caller's environment reaches it and neither replace refs, a grafts file nor a commit-graph
    file are read, so the repository and its objects are the ones this file lies in, and every
    commit's parents are read from the commit itself."""
    try:
        done = subprocess.run(["git", "--no-replace-objects", "-c", "core.commitGraph=false", "-C", str(SCRIPTS), *arguments],
                              stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
                              env=git_environment(), check=False, timeout=timeout)
    except (OSError, subprocess.TimeoutExpired):
        return None
    return done.stdout if done.returncode == 0 else None


def committed_sources():
    """(the commit of this tool's checkout, the committed bytes of this tool, of the receipt check and
    of the candidate tool). Refuses unless each is a file of the checkout that is byte for byte what
    the commit holds at its path."""
    top, head = git("rev-parse", "--show-toplevel"), git("rev-parse", "--verify", "HEAD^{commit}")
    commit = (head or b"").decode("ascii", "replace").strip()
    if top is None or not COMMIT.match(commit):
        raise Refused(["commit: this tool does not run from a git checkout with a commit"])
    top, sources = Path(os.fsdecode(top.rstrip(b"\r\n"))).resolve(), {}
    for path in (TOOL, CHECK_FILE, CANDIDATE_FILE):
        try:
            relative, raw = path.relative_to(top).as_posix(), path.read_bytes()
        except (OSError, ValueError):
            relative = raw = None
        if raw is None or git("cat-file", "blob", f"{commit}:{relative}") != raw:
            raise Refused([f"commit: scripts/{path.name} is not a file that this tool's checkout tracks, "
                           "byte for byte as its commit holds it"])
        sources[path] = raw
    return commit, sources


def load(name, path, raw):
    """A module made from exactly these committed bytes of a file of this checkout."""
    module = types.ModuleType(name)
    module.__file__ = str(path)
    sys.modules[name] = module
    exec(compile(raw, str(path), "exec", dont_inherit=True), module.__dict__)
    return module


def default_branch_tip(repository):
    """(the name of the default branch, the commit at its tip) as the release repository answers now;
    None when it gives no such answer. This one call reads no git configuration: not the system's,
    not the user's and, run in an empty directory outside every repository, not a repository's. So no
    rewrite rule and no other setting of a configuration can send the question elsewhere. The empty
    directory is made below the temporary directory, which is named as the ceiling of git's search
    for a repository; that ceiling is a list the path separator divides, so a temporary directory
    whose path holds the separator cannot be named there and gives no answer."""
    try:
        with tempfile.TemporaryDirectory(prefix="release-upload-attach.") as nowhere:
            if os.pathsep in str(Path(nowhere).resolve()):
                return None
            environment = git_environment()
            environment.update({"GIT_CONFIG_NOSYSTEM": "1", "GIT_CONFIG_SYSTEM": os.devnull, "GIT_CONFIG_GLOBAL": os.devnull,
                                "GIT_CEILING_DIRECTORIES": str(Path(nowhere).resolve().parent), "HOME": nowhere,
                                "XDG_CONFIG_HOME": nowhere})
            done = subprocess.run(["git", "--no-replace-objects", "ls-remote", "--symref", repository, "HEAD"], cwd=nowhere,
                                  stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
                                  env=environment, check=False, timeout=NETWORK_SECONDS)
    except (OSError, subprocess.TimeoutExpired):
        return None
    names, tips = DEFAULT_BRANCH.findall(done.stdout), DEFAULT_TIP.findall(done.stdout)
    if done.returncode != 0 or len(names) != 1 or len(tips) != 1:
        return None
    branch = names[0].decode("utf-8", "replace")
    return (branch, tips[0].decode("ascii")) if BRANCH.match(branch) and ".." not in branch else None


def require_reviewed_commit(commit, repository):
    """Refuse unless commit is on the default branch of the release repository, as the repository
    names its tip now. Returns the branch's name.

    The tip is the one default_branch_tip was told, never a ref a fetch names. The fetch only brings
    that tip's objects into this checkout's object store: it writes no ref and no FETCH_HEAD, and
    moves no remote-tracking ref (a checkout that is a worktree of a shared repository stays as it
    was). A fetch that a configuration sends elsewhere does not bring the tip, or brings it with the
    history it really has; either way only a commit of the true tip's history passes."""
    answer = default_branch_tip(repository)
    if answer is None:
        raise Refused(["commit: the default branch of the release repository is not known"])
    branch, tip = answer
    fetch = ("fetch", "--quiet", "--no-tags", "--no-write-fetch-head", "--refmap=")
    if git(*fetch, repository, f"refs/heads/{branch}", timeout=NETWORK_SECONDS) is None:
        raise Refused([f"commit: the default branch {branch} of the release repository could not be fetched"])
    if git("rev-parse", "--is-shallow-repository") != b"false\n":
        if git(*fetch, "--unshallow", repository, f"refs/heads/{branch}", timeout=NETWORK_SECONDS) is None \
                or git("rev-parse", "--is-shallow-repository") != b"false\n":
            raise Refused(["commit: the history of this tool's checkout could not be completed"])
    if git("cat-file", "-e", tip + "^{commit}") is None or git("merge-base", "--is-ancestor", commit, tip) is None:
        raise Refused([f"commit: {commit} is not on the default branch {branch} of the release repository; "
                       "files are attached only from a commit of that branch"])
    return branch


class GitHub:
    """Every access of this tool to GitHub, through the candidate tool's one `gh api` runner: reads, the
    upload of one new asset to an existing release, and the read-back of an asset's bytes. No request
    here creates, edits or deletes a release or an asset. The upload and the read-back move a whole
    file and have the candidate tool's longer time bound; a read or a listing keeps the short one."""

    def __init__(self, candidate):
        self.candidate = candidate

    def read(self, endpoint):
        return self.candidate.publication_api(endpoint)

    def upload(self, release_id, name, stream):
        return self.candidate.release_asset_upload(release_id, name, stream)

    def download(self, asset_id, stream):
        subprocess.run(["gh", "api", f"repos/{self.candidate.PUBLICATION_REPO}/releases/assets/{asset_id}",
                        "--hostname", "github.com", "--header", "Accept: application/octet-stream"],
                       stdin=subprocess.DEVNULL, stdout=stream, check=True, timeout=self.candidate.UPLOAD_SECONDS)


def staged_rows(staging, check, receipts):
    """The rows of the receipts, in the receipts' order. Refuses unless the entries of the staging
    directory are exactly those rows, each a regular file."""
    problems, rows = [], {}
    for target, receipt in receipts:
        try:
            listed = check.receipt_rows(receipt)
        except check.Refused as error:
            problems += [f"{target}: {reason}" for reason in error.reasons] or [f"{target}: the receipt has no rows"]
            continue
        except Exception as error:
            problems.append(f"{target}: the receipt's rows could not be read ({type(error).__name__})")
            continue
        for name, size, digest in listed:
            if not isinstance(name, str) or not NAME.match(name):
                problems.append(f"{target}: a row's name is not a plain file name")
            elif name in rows:
                problems.append(f"staging: {name} is a row of two receipts ({rows[name].target} and {target})")
            else:
                rows[name] = Row(target, name, size, digest)
    try:
        with os.scandir(staging) as entries:
            found = {entry.name: stat.S_ISREG(entry.stat(follow_symlinks=False).st_mode) for entry in entries}
    except OSError:
        raise Refused(problems + ["staging: the staging directory could not be read"]) from None
    for name in sorted(found):
        if not found[name]:
            problems.append(f"staging: {name} is not a regular file")
        elif name not in rows:
            problems.append(f"staging: {name} is no row of a receipt")
    problems += [f"staging: no file for the row {name}" for name in rows if name not in found]
    if not rows and not problems:
        problems.append("staging: the receipts have no row")
    if problems:
        raise Refused(problems)
    return list(rows.values())


def custody_copy(candidate, source, destination, maximum):
    """Copy a regular file into custody, once, read-only. Returns its name, size and sha256."""
    destination.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
    with destination.open("xb") as retained:
        binding = candidate.copy_publication_input(source, retained, maximum)
    destination.chmod(0o400)
    return binding


def checked(check, target, receipt, commit, files, rows, version, head):
    """The reasons the receipt check does not pass for exactly these rows; none when it passes. The
    check reads the commit of its checkout on its own and takes the pins from it: a pass counts
    only when that commit is head, the one this tool asked the reviewed-commit rule for."""
    try:
        passed = check.check(target, receipt, commit, files, version=version)
    except check.Refused as error:
        return [f"{target}: {reason}" for reason in error.reasons] or [f"{target}: the receipt check refused"]
    except check.Usage as error:
        return [f"{target}: {error}"]
    except Exception as error:
        return [f"{target}: the receipt check did not pass ({type(error).__name__})"]
    try:
        exact = (passed.line.startswith("UPLOAD-CHECK PASS ") and passed.target == target
                 and passed.release_commit == commit and passed.pins_commit == head
                 and tuple(tuple(row) for row in passed.files) == tuple((row.name, row.size, row.sha256) for row in rows))
    except (AttributeError, TypeError):
        exact = False
    return [] if exact else [f"{target}: the receipt check's pass is not for exactly these rows and this commit"]


def read_back(github, asset, row, path):
    """Refuse unless the asset of the release reads back with exactly the row's size and sha256."""
    known = asset.get("digest")
    if asset["name"] != row.name or asset["size"] != row.size or asset["state"] != "uploaded" \
            or known not in (None, "sha256:" + row.sha256):
        raise ValueError(f"the asset {row.name} of the release is not its row's bytes")
    path.parent.mkdir(mode=0o700, exist_ok=True)
    with path.open("xb") as stream:
        github.download(asset["id"], stream)
    digest, size = hashlib.sha256(), 0
    with path.open("rb") as stream:
        while block := stream.read(1 << 20):
            size += len(block)
            digest.update(block)
    path.unlink()
    if size != row.size or digest.hexdigest() != row.sha256:
        raise ValueError(f"the asset {row.name} of the release does not read back as its row's bytes")


def transfer(candidate, github, tag, commit, rows, custody, done):
    """Steps d and e. Appends (row, "attached" or "present") to done as each row is verified on the
    release; raises ValueError with the reason when a step refuses."""
    def require(ok, message):
        if not ok:
            raise ValueError(message)

    repository = candidate.PUBLICATION_REPO
    try:
        found = github.read(f"repos/{repository}/releases/tags/{tag}")
    except Exception:
        found = None
    require(isinstance(found, dict) and type(found.get("id")) is int and found["id"] > 0
            and found.get("tag_name") == tag and found.get("draft") is False,
            f"no published release of the tag {tag} could be read")
    request = {"release_id": found["id"], "tag": tag, "source_sha": commit}

    def state():
        release, assets = candidate.release_state(request, api=github.read)
        require(release["draft"] is False, "the release is a draft")
        return assets

    def unchanged(assets, *earlier):
        return all(assets.get(name) == asset for known in earlier for name, asset in known.items())

    candidate_files = {name for target in candidate.TARGETS for name in candidate.publication_files(target)}
    by_name = {row.name: row for row in rows}

    def foreign(assets):
        return sorted(name for name in assets if name not in by_name and name not in candidate_files)

    # Before the first write: nothing foreign is on the release, and an asset that is already
    # there under a row's name is that row's bytes. Nothing is replaced.
    original = state()
    require(not foreign(original), "the release holds an asset that is neither a row of a checked receipt nor a "
            "candidate file: " + ", ".join(foreign(original)) + "; nothing was uploaded")
    for row in rows:
        if row.name in original:
            read_back(github, original[row.name], row, custody / "read-back" / row.name)
    verified = {}
    for row in rows:
        current = state()
        require(unchanged(current, original, verified), "the assets of the release changed during the run")
        if row.name in original:
            how = "present"
        else:
            require(row.name not in current, f"an asset named {row.name} appeared during the run; nothing is replaced")
            # The anonymous file stays open through the upload: the bytes that leave are the
            # custody copy's, compared with the row once more as they are read.
            with tempfile.TemporaryFile("w+b", dir=custody) as upload:
                require(candidate.copy_publication_input(custody / "files" / row.name, upload, candidate.MAX_FILE)
                        == row.binding(), f"the custody copy of {row.name} changed before its upload")
                upload.seek(0)
                github.upload(found["id"], row.name, upload)
            current = state()
            require(row.name in current, f"the uploaded asset {row.name} was not observed on the release")
            read_back(github, current[row.name], row, custody / "read-back" / row.name)
            how = "attached"
        verified[row.name] = current[row.name]
        done.append((row, how))
    final = state()
    require(unchanged(final, original, verified), "the assets of the release changed during the read-back")
    require(not foreign(final), "the release holds an asset that is neither a row of a receipt this run checked nor "
            "a candidate file: " + ", ".join(foreign(final)) + "; nothing is deleted")


def attach(staging, tag, receipts, release_commit=None, github=None):
    """Run every step. Returns (rows with how each was attached, the release commit, this checkout's
    commit, the default branch); raises Refused, with the rows done so far, when a step refuses."""
    matched = TAG.match(tag)
    if not matched:
        raise Usage("--tag must be connect-vX.Y.Z")
    if release_commit is not None and not COMMIT.match(release_commit):
        raise Usage("--release-commit must be a full lower-case commit id")
    if not receipts or len({target for target, _ in receipts}) != len(receipts) \
            or not all(TARGET.match(target) for target, _ in receipts):
        raise Usage("one receipt per target is required, as TARGET=RECEIPT")
    if staging.is_symlink() or not staging.is_dir():
        raise Usage("--staging must be a directory")
    # A release is its own files and at least one platform's: neither part goes up alone.
    targets = {target for target, _ in receipts}
    if RELEASE_TARGET not in targets or len(targets) < 2:
        raise Refused(["receipts: a run needs the receipt of `release` and of at least one platform target"])

    # a. The reviewed commit, and the code of that commit.
    head, sources = committed_sources()
    candidate = load("release_upload_attach_candidate", CANDIDATE_FILE, sources[CANDIDATE_FILE])
    branch = require_reviewed_commit(head, REPOSITORY % candidate.PUBLICATION_REPO)
    check = load("release_upload_attach_check", CHECK_FILE, sources[CHECK_FILE])
    if not all(hasattr(check, name) for name in CHECK_ENTRY):
        raise Refused(["check: scripts/release-upload-receipt-check.py does not check exact rows"])
    commit = head if release_commit is None else release_commit

    try:
        custody = Path(tempfile.mkdtemp(prefix=".release-upload-attach.", dir=staging.resolve().parent))
    except OSError:
        raise Refused(["custody: no private directory could be made beside the staging directory"]) from None
    done = []
    try:
        # The receipts are read once, from custody copies that keep their file names.
        copies = []
        for target, receipt in receipts:
            try:
                custody_copy(candidate, receipt, custody / "receipts" / target / receipt.name, candidate.MAX_METADATA)
            except (OSError, ValueError):
                raise Refused([f"{target}: the receipt is not a readable regular file"]) from None
            copies.append((target, custody / "receipts" / target / receipt.name))
        # b. The staging directory is exactly the rows.
        rows = staged_rows(staging, check, copies)
        # c. Custody copies of the files, each its row's bytes, and the check on those copies.
        problems = []
        for row in rows:
            try:
                binding = custody_copy(candidate, staging / row.name, custody / "files" / row.name, candidate.MAX_FILE)
            except (OSError, ValueError):
                binding = None
            if binding != row.binding():
                problems.append(f"custody: {row.name} is not its row's size and sha256")
        if problems:
            raise Refused(problems)
        for target, receipt in copies:
            mine = [row for row in rows if row.target == target]
            problems += checked(check, target, receipt, commit, [custody / "files" / row.name for row in mine], mine,
                                matched.group(1), head)
        if problems:
            raise Refused(problems)
        # d, e. Only now is GitHub read or written.
        try:
            transfer(candidate, GitHub(candidate) if github is None else github, tag, commit, rows, custody, done)
        except ValueError as error:
            raise Refused([f"release: {error}"], done) from None
        except Exception as error:
            raise Refused([f"release: a request to the release repository failed ({type(error).__name__})"], done) from None
        return done, commit, head, branch
    finally:
        shutil.rmtree(custody, ignore_errors=True)


def row_line(row, how):
    return f"{how} {row.target} {row.name} {row.sha256} {row.size}"


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter,
                                     allow_abbrev=False)
    parser.add_argument("--staging", type=Path, required=True, help="the directory that holds exactly the files to attach")
    parser.add_argument("--tag", required=True, help="the tag of the existing release, connect-vX.Y.Z")
    parser.add_argument("--receipt", action="append", required=True, metavar="TARGET=RECEIPT",
                        help="the upload receipt of one target's files; once per target")
    parser.add_argument("--release-commit", help="the full id of the commit being released (default: this checkout's)")
    args = parser.parse_args(argv)
    try:
        receipts = []
        for value in args.receipt:
            target, separator, path = value.partition("=")
            if not separator or not path:
                raise Usage("--receipt takes TARGET=RECEIPT")
            receipts.append((target, Path(path)))
        done, commit, head, branch = attach(args.staging, args.tag, receipts, args.release_commit)
    except Usage as error:
        print(f"release-upload-attach: usage error: {error}", file=sys.stderr)
        return 2
    except Refused as error:
        for reason in error.reasons:
            print(reason)
        for row, how in error.done:
            print(row_line(row, how))
        print("RELEASE-ATTACH REFUSED tag=%s attached=%d present=%d: %d reason(s)" % (
            args.tag, sum(how == "attached" for _, how in error.done), sum(how == "present" for _, how in error.done),
            len(error.reasons)))
        return 1
    for row, how in done:
        print(row_line(row, how))
    print("RELEASE-ATTACH PASS tag=%s files=%d attached=%d present=%d release_commit=%s tool_commit=%s default_branch=%s" % (
        args.tag, len(done), sum(how == "attached" for _, how in done), sum(how == "present" for _, how in done),
        commit, head, branch))
    return 0


if __name__ == "__main__":
    sys.exit(main())
