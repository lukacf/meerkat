"""The one tool that attaches lane files to a Connect release: release-upload-attach.py.

Everything here is made up: file names, bytes, commits, a release held in memory. No test contacts
GitHub or needs a token. The tool's GitHub adapter is replaced by a fake release, and the release
repository it asks for its default branch is a scratch repository on disk: every row gives the tool
that repository's address in place of the fixed one. Two guards stand behind that: each made-up
checkout allows no transport but a local file, and every proxy variable of this process names a
closed local port while the tool runs, so a request to a real address fails at once.

The tool runs the receipt check of its own checkout. Most rows give it a stand-in check with the
check's entry points (exact rows, the release commit, the version the names are held to), so that
they pin what the tool does with a verdict. The last class runs the repository's own check.
"""
from pathlib import Path
import ast
import contextlib
import hashlib
import importlib.util
import inspect
import io
import json
import os
import re
import shutil
import stat
import subprocess
import sys
import tempfile
import time
import unittest
import urllib.parse

sys.dont_write_bytecode = True
PROXY_VARIABLES = ("https_proxy", "HTTPS_PROXY", "http_proxy", "HTTP_PROXY", "all_proxy", "ALL_PROXY")


@contextlib.contextmanager
def tool_environment(**changes):
    """This process's environment with these variables set (None: unset) and, while the tool runs,
    every proxy variable naming a closed local port. Restored afterwards."""
    changes = {**dict.fromkeys(PROXY_VARIABLES, "http://127.0.0.1:9"), "no_proxy": None, "NO_PROXY": None, **changes}
    kept = {name: os.environ.get(name) for name in changes}
    try:
        for name, value in changes.items():
            os.environ.pop(name, None) if value is None else os.environ.__setitem__(name, value)
        yield
    finally:
        for name, value in kept.items():
            os.environ.pop(name, None) if value is None else os.environ.__setitem__(name, value)



HERE = Path(__file__).resolve().parent
TOOL = HERE / "release-upload-attach.py"
CANDIDATE = HERE / "connect-candidate.py"
REAL_CHECK = HERE / "release-upload-receipt-check.py"
RELEASE_FILES = HERE.parent / "release"
VERSION = "0.0.0"
TAG = "connect-v" + VERSION
PLATFORM, RELEASE = "linux-x86_64", "release"
GIT = ("-c", "user.name=test", "-c", "user.email=test@example.invalid", "-c", "commit.gpgsign=false",
       "-c", "core.hooksPath=/dev/null", "-c", "init.defaultBranch=main", "-c", "protocol.file.allow=always")
CUSTODY = ".release-upload-attach."
# The files of a made-up release: six that name the platform, five that name none.
PLATFORM_FILES = {f"madeup-{VERSION}-{PLATFORM}.tar.gz": b"made-up product archive\n",
                  f"madeup-{VERSION}-{PLATFORM}.tar.gz.sha256": b"made-up checksum line one\n",
                  f"madeup-gateway-{VERSION}-{PLATFORM}.tar.gz": b"made-up gateway archive\n",
                  f"madeup-gateway-{VERSION}-{PLATFORM}.tar.gz.sha256": b"made-up checksum line two\n",
                  f"madeup-{VERSION}-{PLATFORM}.evidence.json": b'{"made-up": "evidence record"}\n',
                  f"madeup-{VERSION}-{PLATFORM}.evidence.tar.gz": b"made-up evidence archive\n"}
RELEASE_LEVEL_FILES = {"RELEASE-MANIFEST.json": b'{"made-up": "manifest"}\n', "SHA256SUMS": b"made-up sums\n",
                       "source-manifest.json": b'{"made-up": "source manifest"}\n',
                       f"madeup_toolkit-{VERSION}-py3-none-any.whl": b"made-up wheel\n",
                       f"madeup-toolkit-{VERSION}.tgz": b"made-up package tarball\n"}

# A stand-in for the receipt check, with the check's entry points. It passes a receipt for exactly
# its rows, for the commit the receipt names, and holds `release` names to a version. Like the
# check, it states with a pass the commit of the checkout it lies in.
STAND_IN_CHECK = '''"""A stand-in for the receipt check: made-up receipts, the same entry points."""
import hashlib
import json
import os
import subprocess
from pathlib import Path
from typing import NamedTuple

CALLS = []


def checkout_commit():
    """The commit of the checkout this file lies in, read now."""
    environment = {key: value for key, value in os.environ.items() if not key.startswith("GIT_")}
    done = subprocess.run(["git", "-C", str(Path(__file__).resolve().parent), "rev-parse", "HEAD"], env=environment,
                          capture_output=True, text=True)
    return done.stdout.strip()


class Refused(Exception):
    def __init__(self, reasons):
        super().__init__("; ".join(reasons))
        self.reasons = list(reasons)


class Usage(Exception):
    pass


class Passed(NamedTuple):
    line: str
    target: str
    release_commit: str
    files: tuple
    receipt_sha256: str
    pins_sha256: str
    pins_commit: str


def document(receipt):
    try:
        value = json.loads(Path(receipt).read_bytes())
    except (OSError, ValueError):
        raise Refused(["receipt: the receipt is not readable JSON"]) from None
    if not isinstance(value, dict) or value.get("schema") != "made-up-receipt":
        raise Refused(["receipt: not a receipt"])
    return value


def receipt_rows(receipt):
    rows = tuple((row["name"], row["bytes"], row["sha256"]) for row in document(receipt)["payload"])
    if len({name for name, _, _ in rows}) != len(rows):
        raise Refused(["receipt: a name twice"])
    return rows


def check(target, receipt, release_commit, files, *, version=None, max_age_hours=24.0):
    files = [Path(path) for path in files]
    CALLS.append({"target": target, "receipt": str(receipt), "release_commit": release_commit, "version": version,
                  "files": [str(path) for path in files], "max_age_hours": max_age_hours})
    value, reasons = document(receipt), []
    if "PROOF-ONLY" in Path(receipt).name:
        reasons.append("receipt: the receipt file is named PROOF-ONLY")
    if value["target"] != target:
        reasons.append("receipt: target is not " + target)
    if value["verdict"] not in ("PASS", "PASS-FOR-OTHER-ROWS"):
        reasons.append("privacy scan: status is not PASS")
    if value["tool_commit"] != release_commit:
        reasons.append("privacy scan: tool_commit is not the release commit")
    if value["version"] != version:
        reasons.append("files: --version is not the version these names are held to")
    seen = tuple((path.name, path.stat().st_size, hashlib.sha256(path.read_bytes()).hexdigest()) for path in files)
    if seen != receipt_rows(receipt):
        reasons.append("receipt: the files are not exactly the rows of the receipt")
    if reasons:
        raise Refused(reasons)
    if value["verdict"] == "PASS-FOR-OTHER-ROWS":
        seen = seen[:-1]
    # A receipt may say what the pass states in place of the caller's commit and the checkout's.
    stated = value.get("pass_states", {})
    return Passed("UPLOAD-CHECK PASS files=%d" % len(seen), target, stated.get("release_commit", release_commit), seen,
                  "0" * 64, "0" * 64, stated.get("pins_commit", checkout_commit()))
'''
# The check as it was before it had an entry point for exact rows: a command line only.
FORMER_CHECK = '"""A stand-in for a receipt check without the exact-rows entry points."""\n\n\ndef main():\n    return 0\n'


def load(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    value = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(value)
    return value


candidate = load("candidate_constants_for_the_attach_rows", CANDIDATE)
REPOSITORY = "https://github.com/%s.git" % candidate.PUBLICATION_REPO
CANDIDATE_FILES = [name for target in candidate.TARGETS for name in candidate.publication_files(target)]


def sha256(data):
    return hashlib.sha256(data).hexdigest()


def git(directory, *arguments, check=True):
    environment = {key: value for key, value in os.environ.items() if not key.startswith("GIT_")}
    done = subprocess.run(["git", "-C", str(directory), *GIT, *arguments], capture_output=True, text=True, env=environment)
    if check and done.returncode != 0:
        raise AssertionError("git %s: %s" % (arguments[0], done.stderr))
    return done.stdout.strip()


def commit(directory, text):
    (directory / "file.txt").write_text(text)
    git(directory, "add", "-A")
    git(directory, "commit", "-q", "-m", text)
    return git(directory, "rev-parse", "HEAD")


def release_repository(root, check=STAND_IN_CHECK, files=()):
    """A made-up release repository at root/origin.git. Its default branch main has three commits, the
    first of which holds the tool, the candidate tool and a receipt check; `old` names main's first
    commit and `side` left main there and was never merged. Returns (repository, commits)."""
    seed = root / "seed"
    scripts = seed / "connect/scripts"
    scripts.mkdir(parents=True)
    shutil.copyfile(TOOL, scripts / TOOL.name)
    shutil.copyfile(CANDIDATE, scripts / CANDIDATE.name)
    if isinstance(check, Path):
        shutil.copyfile(check, scripts / REAL_CHECK.name)
    else:
        (scripts / REAL_CHECK.name).write_text(check)
    for relative, text in files:
        (seed / relative).parent.mkdir(parents=True, exist_ok=True)
        (seed / relative).write_text(text)
    git(seed, "init", "-q")
    first = commit(seed, "first")
    git(seed, "branch", "old", first)
    git(seed, "checkout", "-q", "-b", "side")
    side = commit(seed, "side work")
    git(seed, "checkout", "-q", "main")
    second = commit(seed, "second")
    tip = commit(seed, "third")
    bare = root / "origin.git"
    git(root, "clone", "-q", "--bare", str(seed), str(bare))
    return bare, {"first": first, "side": side, "second": second, "tip": tip}


def address(repository):
    """The value these rows give the tool's REPOSITORY, its one place for the release repository's
    address: a scratch repository on disk. The tool fills the candidate tool's repository name into
    that text; the last four characters take the name and print nothing of it."""
    return "file://" + str(repository).replace("%", "%%") + "%.0s"


def local_only(checkout):
    """No transport but a local file is allowed in checkout: a fetch of a real address fails there."""
    git(checkout, "config", "--local", "protocol.allow", "never")
    git(checkout, "config", "--local", "protocol.file.allow", "always")


def clone(root, repository, name, *options):
    git(root, "clone", "-q", *options, "file://" + str(repository), str(root / name))
    local_only(root / name)
    return root / name


def rewrite_commit_graph_parent(checkout, child, parent):
    """In the commit-graph file of a checkout, make parent the first parent of child and set the file's
    trailing checksum again. The commits themselves stay as they are."""
    path = checkout / ".git" / "objects" / "info" / "commit-graph"
    data = bytearray(path.read_bytes())
    assert data[:4] == b"CGPH", "not a commit-graph file"
    size = 20 if data[5] == 1 else 32                        # the length of a commit id in this file
    chunks = {}
    for index in range(data[6] + 1):                          # the table of chunks: a name and an offset each
        place = 8 + 12 * index
        chunks[bytes(data[place:place + 4])] = int.from_bytes(data[place + 4:place + 12], "big")
    count = int.from_bytes(data[chunks[b"OIDF"] + 255 * 4:chunks[b"OIDF"] + 256 * 4], "big")
    known = [bytes(data[chunks[b"OIDL"] + index * size:chunks[b"OIDL"] + (index + 1) * size]).hex() for index in range(count)]
    row = chunks[b"CDAT"] + known.index(child) * (size + 16)  # a commit's row: its tree, then its first parent
    data[row + size:row + size + 4] = known.index(parent).to_bytes(4, "big")
    data[-size:] = (hashlib.sha1 if size == 20 else hashlib.sha256)(bytes(data[:-size])).digest()
    rewritten = path.with_name("commit-graph.rewritten")
    rewritten.write_bytes(bytes(data))
    os.replace(rewritten, path)


class FakeRelease:
    """A made-up release held in memory, with the three calls of the tool's GitHub adapter. Never GitHub."""

    def __init__(self, commit, tag=TAG):
        self.repository, self.tag, self.commit, self.id = candidate.PUBLICATION_REPO, tag, commit, 77
        self.assets, self.next_id = {}, 100
        self.exists, self.draft, self.draft_by_id, self.private, self.digests = True, False, False, False, True
        self.reads, self.uploads, self.downloads = [], [], []
        self.fail_upload = self.stores_other_bytes = self.after_upload = self.before_read = self.after_download = None
        self.states = {}

    def add(self, name, data):
        self.assets[name] = {"id": self.next_id, "data": data}
        self.next_id += 1

    def row(self, name):
        asset = self.assets[name]
        return {"id": asset["id"], "name": name, "size": len(asset["data"]), "state": self.states.get(name, "uploaded"),
                "browser_download_url": f"https://github.com/{self.repository}/releases/download/{self.tag}/{name}",
                "digest": "sha256:" + sha256(asset["data"]) if self.digests else None, "download_count": len(self.downloads)}

    def read(self, endpoint):
        if self.before_read is not None:
            self.before_read(endpoint)
        self.reads.append(endpoint)
        base = "repos/" + self.repository
        if endpoint == base:
            return {"full_name": self.repository, "private": self.private, "visibility": "private" if self.private else "public"}
        if endpoint == f"{base}/releases/tags/{self.tag}":
            if not self.exists:
                raise subprocess.CalledProcessError(1, "made-up: no release of the tag")
            return {"id": self.id, "tag_name": self.tag, "draft": self.draft}
        if endpoint == f"{base}/releases/{self.id}":
            return {"id": self.id, "tag_name": self.tag, "draft": self.draft or self.draft_by_id}
        if endpoint == f"{base}/git/ref/tags/{self.tag}":
            return {"object": {"type": "commit", "sha": self.commit, "url": "made-up"}}
        if endpoint.startswith(f"{base}/releases/{self.id}/assets?"):
            page = int(urllib.parse.parse_qs(urllib.parse.urlsplit(endpoint).query)["page"][0])
            return [self.row(name) for name in self.assets][(page - 1) * 100:page * 100]
        raise AssertionError("a read the tool has no reason for: " + endpoint)

    def upload(self, release_id, name, stream):
        self.uploads.append(name)
        assert release_id == self.id and name not in self.assets, "a release holds one asset of a name"
        if name == self.fail_upload:
            raise OSError("made-up upload failure")
        data = stream.read()
        self.add(name, data + b"other" if name == self.stores_other_bytes else data)
        if self.after_upload is not None:
            self.after_upload(name)

    def download(self, asset_id, stream):
        self.downloads.append(asset_id)
        stream.write(next(asset["data"] for asset in self.assets.values() if asset["id"] == asset_id))
        if self.after_download is not None:
            self.after_download(asset_id)


class Fixture(unittest.TestCase):
    check = STAND_IN_CHECK
    files = ()

    @classmethod
    def setUpClass(cls):
        cls.base = Path(tempfile.mkdtemp(prefix="attach-repository-")).resolve()
        cls.addClassCleanup(shutil.rmtree, cls.base, True)
        cls.repository, cls.commits = release_repository(cls.base, cls.check, cls.files)

    def setUp(self):
        self.root = Path(tempfile.mkdtemp(prefix="attach-test-")).resolve()
        self.addCleanup(shutil.rmtree, self.root, True)
        self.count = 0
        self.checkout = clone(self.root, self.repository, "checkout")
        self.head = self.commits["tip"]
        self.staging = self.root / "lane" / "staging"
        self.staging.mkdir(parents=True)
        for name, data in {**PLATFORM_FILES, **RELEASE_LEVEL_FILES}.items():
            (self.staging / name).write_bytes(data)
        self.release = FakeRelease(self.head)

    def rows(self, names, directory=None):
        directory = self.staging if directory is None else directory
        return [{"name": name, "bytes": (directory / name).stat().st_size, "sha256": sha256((directory / name).read_bytes())}
                for name in names]

    def receipt(self, target, names=None, name=None, **changes):
        """A made-up receipt of the stand-in check for the staged files of target."""
        names = list(PLATFORM_FILES if target == PLATFORM else RELEASE_LEVEL_FILES) if names is None else names
        value = {"schema": "made-up-receipt", "target": changes.pop("states_target", target), "version": VERSION,
                 "payload": self.rows(names), "verdict": "PASS", "tool_commit": self.head, **changes}
        self.count += 1
        path = self.root / "lane" / (name or f"receipt-{target}-{self.count}.json")
        path.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n")
        return path

    def receipts(self):
        return [(PLATFORM, self.receipt(PLATFORM)), (RELEASE, self.receipt(RELEASE))]

    def tool(self, checkout=None):
        self.count += 1
        return load(f"attach_tool_{id(self)}_{self.count}", (checkout or self.checkout) / "connect/scripts" / TOOL.name)

    def run_tool(self, receipts=None, *extra, checkout=None, release=None, tag=TAG, staging=None, module=None,
                 repository=None, variables=None, temporary=None):
        """Run the tool's command in this process, from a checkout, against the fake release and with a
        scratch repository as the release repository. Returns (exit status, the lines it printed, its
        error text). With temporary, that directory is the temporary directory while the tool runs."""
        module = self.tool(checkout) if module is None else module
        release = self.release if release is None else release
        module.GitHub = lambda _candidate: release
        module.REPOSITORY = address(self.repository if repository is None else repository)
        sys.modules.pop("release_upload_attach_check", None)
        arguments = ["--staging", str(self.staging if staging is None else staging), "--tag", tag]
        for target, path in (self.receipts() if receipts is None else receipts):
            arguments += ["--receipt", f"{target}={path}"]
        out, err = io.StringIO(), io.StringIO()
        kept = tempfile.tempdir
        tempfile.tempdir = kept if temporary is None else str(temporary)
        try:
            with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err), tool_environment(**(variables or {})):
                try:
                    code = module.main([*arguments, *extra])
                except SystemExit as error:
                    code = error.code
        finally:
            tempfile.tempdir = kept
        self.module = module
        return code, out.getvalue().splitlines(), err.getvalue()

    def check_calls(self):
        """The calls of the receipt check in the last run; none when the run did not load the check."""
        loaded = sys.modules.get("release_upload_attach_check")
        return [] if loaded is None else loaded.CALLS

    def assert_nothing_reached_github(self, release=None):
        release = self.release if release is None else release
        self.assertEqual((release.reads, release.uploads, release.downloads), ([], [], []))

    def assert_refused(self, result, *words, release=None, written=()):
        code, lines, _ = result
        self.assertEqual(code, 1, lines)
        self.assertTrue(lines and lines[-1].startswith("RELEASE-ATTACH REFUSED "), lines)
        for word in words:
            self.assertIn(word, "\n".join(lines))
        self.assertEqual((self.release if release is None else release).uploads, list(written))

    def assert_no_custody_left(self):
        self.assertEqual([path.name for path in self.staging.parent.iterdir() if path.name.startswith(CUSTODY)], [])


class ReviewedCommit(Fixture):
    """Step a. The files are attached only from a commit of the release repository's default branch,
    and the code that runs is that commit's."""

    def test_a_commit_of_the_default_branch_attaches_and_any_other_commit_refuses(self):
        code, lines, _ = self.run_tool()
        self.assertEqual(code, 0, lines)
        self.assertIn(f"tool_commit={self.commits['tip']} default_branch=main", lines[-1])
        # An earlier commit of the default branch, in a full and in a shallow checkout: the history
        # is fetched, not assumed.
        self.head = self.commits["first"]
        earlier = clone(self.root, self.repository, "earlier")
        git(earlier, "checkout", "-q", "--detach", self.commits["first"])
        self.assertEqual(self.run_tool(checkout=earlier, release=FakeRelease(self.head))[0], 0)
        shallow = clone(self.root, self.repository, "shallow", "--depth", "1", "--branch", "old")
        self.assertEqual(git(shallow, "rev-parse", "--is-shallow-repository"), "true")
        self.assertEqual(self.run_tool(checkout=shallow, release=FakeRelease(self.head))[0], 0)
        self.assertEqual(git(shallow, "rev-parse", "--is-shallow-repository"), "false")
        # A branch that was never merged, full and shallow, and a commit only this checkout has.
        self.head = self.commits["side"]
        for name, options in (("side", ("--branch", "side")), ("side-shallow", ("--depth", "1", "--branch", "side"))):
            side = clone(self.root, self.repository, name, *options)
            self.assertEqual(git(side, "rev-parse", "HEAD"), self.commits["side"])
            release = FakeRelease(self.head)
            self.assert_refused(self.run_tool(checkout=side, release=release), "is not on the default branch main", release=release)
            self.assert_nothing_reached_github(release)
        local = clone(self.root, self.repository, "local")
        self.head = commit(local, "not merged")
        release = FakeRelease(self.head)
        self.assert_refused(self.run_tool(checkout=local, release=release), "is not on the default branch", release=release)
        self.assert_nothing_reached_github(release)
        self.assertEqual(self.check_calls(), [])    # nothing was judged either: the rule comes first

    def test_the_rule_refuses_when_the_branch_is_unknown_or_cannot_be_fetched(self):
        # The repository cannot be reached: an earlier fetch of the branch is not trusted instead.
        self.assertEqual(self.run_tool()[0], 0)
        self.release = FakeRelease(self.head)
        self.assert_refused(self.run_tool(repository=self.root / "gone.git"),
                            "the default branch of the release repository is not known")
        self.assert_nothing_reached_github()
        # The repository names no default branch, or one that it does not have.
        for name, arguments in (("detached", ("update-ref", "--no-deref", "HEAD", self.commits["tip"])),
                                ("unborn", ("symbolic-ref", "HEAD", "refs/heads/no-such-branch"))):
            other = self.root / (name + ".git")
            git(self.root, "clone", "-q", "--bare", str(self.repository), str(other))
            git(other, *arguments)
            self.assert_refused(self.run_tool(repository=other), "commit: the default branch")
            self.assert_nothing_reached_github()
        # The repository names a default branch whose name is not of the one form the tool takes. The
        # branch exists and holds the checkout's commit; the tool still does not know it.
        odd = self.root / "odd.git"
        git(self.root, "clone", "-q", "--bare", str(self.repository), str(odd))
        git(odd, "update-ref", "refs/heads/odd+name", self.commits["tip"])
        git(odd, "symbolic-ref", "HEAD", "refs/heads/odd+name")
        self.assertIn("ref: refs/heads/odd+name\tHEAD", git(self.root, "ls-remote", "--symref", "file://" + str(odd), "HEAD"))
        self.assert_refused(self.run_tool(repository=odd), "commit: the default branch of the release repository is not known")
        self.assert_nothing_reached_github()
        # The branch is known but its fetch fails.
        module = self.tool()
        real = module.git
        module.git = lambda *arguments, **options: None if arguments[0] == "fetch" else real(*arguments, **options)
        self.assert_refused(self.run_tool(module=module), "the default branch main of the release repository could not be fetched")
        self.assert_nothing_reached_github()
        # Not a checkout at all.
        plain = self.root / "plain"
        shutil.copytree(self.checkout / "connect", plain / "connect")
        self.assert_refused(self.run_tool(checkout=plain), "commit: ")
        self.assert_nothing_reached_github()

    def other_repository(self):
        """A second scratch repository whose main holds a commit the release repository does not have,
        and a checkout of it that stands on that commit. Returns (repository, checkout)."""
        fork = self.root / "fork.git"
        git(self.root, "clone", "-q", "--bare", str(self.repository), str(fork))
        work = self.root / "work"
        git(self.root, "clone", "-q", "file://" + str(fork), str(work))
        self.head = commit(work, "merged in the other repository alone")
        git(work, "push", "-q", "origin", "main")
        local_only(work)
        self.assertEqual(git(work, "rev-parse", "refs/remotes/origin/main"), self.head)
        return fork, work

    def test_the_rule_asks_the_release_repository_and_not_the_checkouts_own_remote(self):
        # The checkout's remote would vouch for the commit the checkout stands on.
        _, work = self.other_repository()
        release = FakeRelease(self.head)
        self.assert_refused(self.run_tool(checkout=work, release=release), "is not on the default branch main", release=release)
        self.assert_nothing_reached_github(release)

    def test_a_rewrite_rule_of_a_git_configuration_cannot_name_another_repository(self):
        # A rule that sends every request for the release repository's address to the other repository:
        # in the checkout's own configuration, and in the user's configuration alone.
        fork, work = self.other_repository()
        rule = ("url.file://%s.insteadOf" % fork, "file://" + str(self.repository))
        home = self.root / "home"
        home.mkdir()
        (home / ".gitconfig").write_text('[url "file://%s"]\n\tinsteadOf = file://%s\n' % (fork, self.repository))
        for label, variables in (("the checkout's configuration", {}),
                                 ("the user's configuration", {"HOME": str(home), "XDG_CONFIG_HOME": None})):
            with self.subTest(rule=label):
                if not variables:
                    git(work, "config", "--local", *rule)
                    self.assertEqual(git(work, "ls-remote", "file://" + str(self.repository), "refs/heads/main").split()[0], self.head)
                release = FakeRelease(self.head)
                self.assert_refused(self.run_tool(checkout=work, release=release, variables=variables),
                                    "is not on the default branch main", release=release)
                self.assert_nothing_reached_github(release)
                if not variables:
                    git(work, "config", "--local", "--unset-all", rule[0])
        # With the rule in force a commit of the release repository's default branch still passes.
        self.head = self.commits["tip"]
        git(self.checkout, "config", "--local", *rule)
        code, lines, _ = self.run_tool(variables={"HOME": str(home), "XDG_CONFIG_HOME": None})
        self.assertEqual(code, 0, lines)

    def test_a_repository_above_the_temporary_directory_gives_the_question_no_configuration(self):
        # The question runs in an empty directory that the tool makes below the temporary directory,
        # and names that directory as the ceiling of git's search for a repository. Here a repository
        # above it holds a rule that sends every request for the release repository's address to the
        # other repository.
        fork, work = self.other_repository()
        above = self.root / "above"
        above.mkdir()
        git(above, "init", "-q")
        git(above, "config", "--local", "url.file://%s.insteadOf" % fork, "file://" + str(self.repository))
        # The ceiling holds: git does not search above the empty directory, the question reaches the
        # release repository, and the commit that only the other repository holds refuses.
        plain = above / "plain"
        plain.mkdir()
        self.assertEqual(git(plain, "ls-remote", "file://" + str(self.repository), "refs/heads/main").split()[0], self.head)
        release = FakeRelease(self.head)
        self.assert_refused(self.run_tool(checkout=work, release=release, temporary=plain), "is not on the default branch main",
                            release=release)
        self.assert_nothing_reached_github(release)
        # The ceiling is a list that the path separator divides, so a path that holds the separator
        # names no ceiling: git would search above the empty directory and read the rule.
        temporary = above / ("with%scolon" % os.pathsep)
        temporary.mkdir()
        self.assertEqual(git(temporary, "ls-remote", "file://" + str(self.repository), "refs/heads/main").split()[0], self.head)
        release = FakeRelease(self.head)
        self.assert_refused(self.run_tool(checkout=work, release=release, temporary=temporary),
                            "commit: the default branch of the release repository is not known", release=release)
        self.assert_nothing_reached_github(release)
        # The path that is judged is the resolved one, as the ceiling is: a link whose own name holds no
        # separator, to that directory, refuses in the same way.
        link = self.root / "link-to-it"
        link.symlink_to(temporary, target_is_directory=True)
        self.assertNotIn(os.pathsep, str(link))
        release = FakeRelease(self.head)
        self.assert_refused(self.run_tool(checkout=work, release=release, temporary=link),
                            "commit: the default branch of the release repository is not known", release=release)
        self.assert_nothing_reached_github(release)

    def test_the_question_runs_in_an_empty_directory_with_a_home_of_its_own(self):
        # What the one git call that reads no configuration is started with. The two variables that
        # name the null device as the system's and the user's configuration are not read by a git
        # before 2.32; for such a git the home of its own is what keeps the user's configuration out.
        home = self.root / "home"
        (home / "xdg" / "git").mkdir(parents=True)
        module = self.tool()
        real, seen = module.subprocess.run, []

        def run(command, **options):
            if "ls-remote" in command:
                seen.append((list(command), options["cwd"], dict(options["env"]), os.listdir(options["cwd"])))
            return real(command, **options)

        module.subprocess = type("Subprocess", (), {"run": staticmethod(run), "DEVNULL": subprocess.DEVNULL, "PIPE": subprocess.PIPE,
                                                    "TimeoutExpired": subprocess.TimeoutExpired})
        code, lines, _ = self.run_tool(module=module, variables={
            "HOME": str(home), "XDG_CONFIG_HOME": str(home / "xdg"), "GIT_CONFIG_GLOBAL": str(home / ".gitconfig"),
            "GIT_CONFIG_SYSTEM": str(home / ".gitconfig"), "GIT_CONFIG_COUNT": "0", "GIT_DIR": str(self.checkout / ".git")})
        self.assertEqual(code, 0, lines)
        ((command, directory, environment, entries),) = seen
        self.assertEqual(command, ["git", "--no-replace-objects", "ls-remote", "--symref", "file://" + str(self.repository), "HEAD"])
        # An empty directory of the tool's own, below the temporary directory, gone after the call.
        self.assertEqual((entries, os.path.exists(directory)), ([], False))
        above = Path(directory).resolve().parent
        self.assertEqual(above, Path(tempfile.gettempdir()).resolve())
        self.assertEqual({name: value for name, value in environment.items() if name.startswith("GIT_") or name in ("HOME", "XDG_CONFIG_HOME")},
                         {"GIT_TERMINAL_PROMPT": "0", "GIT_GRAFT_FILE": os.devnull, "GIT_CONFIG_NOSYSTEM": "1",
                          "GIT_CONFIG_SYSTEM": os.devnull, "GIT_CONFIG_GLOBAL": os.devnull,
                          "GIT_CEILING_DIRECTORIES": str(above), "HOME": directory, "XDG_CONFIG_HOME": directory})

    def test_the_fetch_brings_objects_and_moves_no_ref_of_the_checkout(self):
        # A release repository of this row alone, whose default branch moves on after the checkout was made.
        true = self.root / "true.git"
        git(self.root, "clone", "-q", "--bare", str(self.repository), str(true))
        checkout = clone(self.root, true, "behind")
        ahead = clone(self.root, true, "ahead")
        newer = commit(ahead, "fourth")
        git(ahead, "push", "-q", "origin", "main")
        before = git(checkout, "for-each-ref")
        self.assertIn("refs/remotes/origin/main", before)
        module = self.tool(checkout)
        calls, real = [], module.git
        module.git = lambda *arguments, **options: calls.append(arguments) or real(*arguments, **options)
        code, lines, _ = self.run_tool(checkout=checkout, module=module, repository=true)
        self.assertEqual(code, 0, lines)
        # The newer tip is in the checkout's object store, and no ref and no FETCH_HEAD names it.
        git(checkout, "cat-file", "-e", newer + "^{commit}")
        self.assertEqual(git(checkout, "for-each-ref"), before)
        self.assertEqual(git(checkout, "rev-parse", "refs/remotes/origin/main"), self.commits["tip"])
        self.assertFalse((checkout / ".git" / "FETCH_HEAD").exists())
        fetches = [arguments for arguments in calls if arguments[0] == "fetch"]
        self.assertEqual(fetches, [("fetch", "--quiet", "--no-tags", "--no-write-fetch-head", "--refmap=",
                                    "file://" + str(true), "refs/heads/main")])

    def test_a_grafts_file_states_no_ancestry(self):
        # A grafts line that names the unmerged commit as the parent of the default branch's tip: read
        # with it, the unmerged commit is an ancestor of the tip.
        side = clone(self.root, self.repository, "side", "--branch", "side")
        git(side, "fetch", "-q", "origin", "main")
        (side / ".git" / "info").mkdir(exist_ok=True)
        (side / ".git" / "info" / "grafts").write_text(f"{self.commits['tip']} {self.commits['side']}\n")
        git(side, "merge-base", "--is-ancestor", self.commits["side"], self.commits["tip"])    # raises unless it is one
        self.head = self.commits["side"]
        release = FakeRelease(self.head)
        self.assert_refused(self.run_tool(checkout=side, release=release), "is not on the default branch main", release=release)
        self.assert_nothing_reached_github(release)
        # A variable of the caller cannot bring a grafts file either.
        self.head = self.commits["tip"]
        plain = clone(self.root, self.repository, "plain-side", "--branch", "side")
        git(plain, "fetch", "-q", "origin", "main")
        self.head = self.commits["side"]
        release = FakeRelease(self.head)
        self.assert_refused(self.run_tool(checkout=plain, release=release, variables={
            "GIT_GRAFT_FILE": str(side / ".git" / "info" / "grafts")}), "is not on the default branch main", release=release)

    def test_a_commit_graph_file_states_no_ancestry(self):
        # git takes the parents of every commit below the ones it is asked about from a commit-graph
        # file when the checkout has one. Here the file's row for the parent of the default branch's
        # tip names the unmerged commit as its parent: read with it, the unmerged commit is an
        # ancestor of the tip.
        side = clone(self.root, self.repository, "side", "--branch", "side")
        git(side, "commit-graph", "write", "--reachable")
        rewrite_commit_graph_parent(side, self.commits["second"], self.commits["side"])
        git(side, "merge-base", "--is-ancestor", self.commits["side"], self.commits["tip"])    # raises unless it is one
        self.head = self.commits["side"]
        release = FakeRelease(self.head)
        self.assert_refused(self.run_tool(checkout=side, release=release), "is not on the default branch main", release=release)
        self.assert_nothing_reached_github(release)

    def test_the_git_calls_ignore_the_callers_environment_and_replace_refs(self):
        # A replace ref that gives the default branch's tip the unmerged commit as its parent: read
        # with replace refs, the unmerged commit is an ancestor of the tip.
        side = clone(self.root, self.repository, "side", "--branch", "side")
        git(side, "fetch", "-q", "origin", "main")
        stand_in = git(side, "commit-tree", "-p", self.commits["side"], "-m", "made-up parent", self.commits["tip"] + "^{tree}")
        git(side, "replace", self.commits["tip"], stand_in)
        git(side, "merge-base", "--is-ancestor", self.commits["side"], self.commits["tip"])    # raises unless it is one
        self.head = self.commits["side"]
        release = FakeRelease(self.head)
        self.assert_refused(self.run_tool(checkout=side, release=release), "is not on the default branch main", release=release)
        # Variables that name another repository, an index or a work tree do not reach the tool's git.
        self.head = self.commits["tip"]
        elsewhere = clone(self.root, self.repository, "elsewhere", "--branch", "side")
        code, lines, _ = self.run_tool(variables={
            "GIT_DIR": str(elsewhere / ".git"), "GIT_WORK_TREE": str(elsewhere), "GIT_INDEX_FILE": str(self.root / "no-index"),
            "GIT_CEILING_DIRECTORIES": str(self.root), "GIT_OBJECT_DIRECTORY": str(self.root / "no-objects")})
        self.assertEqual(code, 0, lines)
        self.assertIn("tool_commit=" + self.commits["tip"], lines[-1])

    def test_the_tool_the_check_and_the_candidate_tool_are_the_commits_files(self):
        for name in (TOOL.name, REAL_CHECK.name, CANDIDATE.name):
            with self.subTest(name=name):
                path = self.checkout / "connect/scripts" / name
                kept = path.read_bytes()
                module = self.tool()
                path.write_bytes(kept + b"\n# edited after the commit\n")
                self.assert_refused(self.run_tool(module=module), f"commit: scripts/{name} is not a file")
                self.assert_nothing_reached_github()
                # Staged, the edit is still not the commit's.
                git(self.checkout, "add", "-A")
                self.assert_refused(self.run_tool(module=module), f"commit: scripts/{name} is not a file")
                path.write_bytes(kept)
                git(self.checkout, "add", "-A")
        self.assertEqual(self.run_tool()[0], 0)

    def test_a_check_without_the_exact_rows_entry_points_refuses(self):
        # A commit of the default branch whose receipt check is of the former form.
        (self.checkout / "connect/scripts" / REAL_CHECK.name).write_text(FORMER_CHECK)
        self.head = commit(self.checkout, "a check of the former form")
        git(self.checkout, "push", "-q", "file://" + str(self.repository), "HEAD:refs/heads/main")
        self.addCleanup(git, self.repository, "update-ref", "refs/heads/main", self.commits["tip"])
        self.release = FakeRelease(self.head)
        self.assert_refused(self.run_tool(), "check: scripts/release-upload-receipt-check.py does not check exact rows")
        self.assert_nothing_reached_github()


class StagingIsExactlyTheRows(Fixture):
    """Step b. What is staged is what the receipts name: no file more, no file less."""

    def test_the_staging_directory_holds_exactly_the_rows_of_the_receipts(self):
        receipts = self.receipts()
        # An entry that is no row of a receipt.
        (self.staging / "madeup-extra.txt").write_bytes(b"made-up extra file\n")
        self.assert_refused(self.run_tool(receipts), "staging: madeup-extra.txt is no row of a receipt")
        (self.staging / "madeup-extra.txt").unlink()
        (self.staging / ".madeup-hidden").write_bytes(b"made-up hidden file\n")
        self.assert_refused(self.run_tool(receipts), "staging: .madeup-hidden is no row of a receipt")
        (self.staging / ".madeup-hidden").unlink()
        # A row without an entry.
        kept = (self.staging / "SHA256SUMS").read_bytes()
        (self.staging / "SHA256SUMS").unlink()
        self.assert_refused(self.run_tool(receipts), "staging: no file for the row SHA256SUMS")
        (self.staging / "SHA256SUMS").write_bytes(kept)
        # A receipt that names fewer files than are staged for its target.
        fewer = [receipts[0], (RELEASE, self.receipt(RELEASE, [name for name in RELEASE_LEVEL_FILES if name != "SHA256SUMS"]))]
        self.assert_refused(self.run_tool(fewer), "staging: SHA256SUMS is no row of a receipt")
        # A name under two receipts.
        shared = [(PLATFORM, self.receipt(PLATFORM, [*PLATFORM_FILES, "SHA256SUMS"])), receipts[1]]
        self.assert_refused(self.run_tool(shared), f"staging: SHA256SUMS is a row of two receipts ({PLATFORM} and {RELEASE})")
        # Receipts without a row, and a row name that is not a plain file name.
        self.assert_refused(self.run_tool([(PLATFORM, self.receipt(PLATFORM, payload=[])), receipts[1]]), "is no row of a receipt")
        odd = dict(self.rows(["SHA256SUMS"])[0], name="../SHA256SUMS")
        self.assert_refused(self.run_tool([receipts[0], (RELEASE, self.receipt(RELEASE, payload=[odd]))]),
                            f"{RELEASE}: a row's name is not a plain file name")
        self.assert_nothing_reached_github()
        self.assertEqual(self.check_calls(), [])
        self.assert_no_custody_left()
        self.assertEqual(self.run_tool(receipts)[0], 0)

    def test_every_staged_entry_is_a_regular_file(self):
        receipts = self.receipts()
        name = "source-manifest.json"
        kept = self.root / "kept-source-manifest.json"
        (self.staging / name).rename(kept)
        for label, make in (("a link to the very bytes", lambda path: path.symlink_to(kept)),
                            ("a directory", lambda path: path.mkdir()),
                            ("a special file", lambda path: os.mkfifo(path))):
            with self.subTest(entry=label):
                make(self.staging / name)
                self.assert_refused(self.run_tool(receipts), f"staging: {name} is not a regular file")
                self.assert_nothing_reached_github()
                self.assertEqual(self.check_calls(), [])
                if (self.staging / name).is_dir() and not (self.staging / name).is_symlink():
                    (self.staging / name).rmdir()
                else:
                    (self.staging / name).unlink()
        # The staging directory itself is a directory, not a link to one.
        link = self.root / "lane" / "staging-link"
        link.symlink_to(self.staging)
        kept.rename(self.staging / name)
        code, _, error = self.run_tool(receipts, staging=link)
        self.assertEqual((code, self.release.reads), (2, []), error)
        self.assertEqual(self.run_tool(receipts)[0], 0)


class TheCheckJudges(Fixture):
    """Step c. The receipt check of the tool's checkout judges every receipt, for exactly its rows, on
    copies the tool holds; nothing reaches GitHub before every receipt passed."""

    def test_each_receipt_is_checked_once_for_exactly_its_rows_on_custody_copies(self):
        receipts = self.receipts()
        code, lines, _ = self.run_tool(receipts)
        self.assertEqual(code, 0, lines)
        calls = self.check_calls()
        self.assertEqual([call["target"] for call in calls], [PLATFORM, RELEASE])
        for call, names in zip(calls, (PLATFORM_FILES, RELEASE_LEVEL_FILES)):
            self.assertEqual([Path(path).name for path in call["files"]], list(names))
            # The files and the receipt given to the check are the tool's own copies, not the staged paths.
            for path in (*call["files"], call["receipt"]):
                self.assertNotIn(str(self.staging), path)
                self.assertIn(str(self.staging.parent / CUSTODY), path)
            self.assertEqual(call["release_commit"], self.head)
            self.assertEqual(call["max_age_hours"], 24.0)
        # The version that every receipt's names are held to is the tag's.
        self.assertEqual([call["version"] for call in calls], [VERSION, VERSION])
        self.assertEqual([Path(call["receipt"]).name for call in calls], [path.name for _, path in receipts])
        self.assert_no_custody_left()

    def test_a_receipt_the_check_refuses_uploads_nothing(self):
        for label, target, changes, word in (
                ("a scan that did not pass", RELEASE, {"verdict": "REFUSED"}, f"{RELEASE}: privacy scan: status is not PASS"),
                ("a scan of another commit", PLATFORM, {"tool_commit": self.commits["second"]},
                 f"{PLATFORM}: privacy scan: tool_commit is not the release commit"),
                ("names held to another version", RELEASE, {"version": "9.9.9"}, f"{RELEASE}: files: --version"),
                ("platform names held to another version", PLATFORM, {"version": "9.9.9"}, f"{PLATFORM}: files: --version"),
                ("a receipt of another target", PLATFORM, {"states_target": "macos-arm64"}, f"{PLATFORM}: receipt: target is not")):
            with self.subTest(case=label):
                receipts = dict(self.receipts())
                receipts[target] = self.receipt(target, **changes)
                self.assert_refused(self.run_tool(list(receipts.items())), word)
                self.assert_nothing_reached_github()
                self.assert_no_custody_left()
        # A receipt file named as a proof run: the check sees the name it was given under.
        named = [(PLATFORM, self.receipt(PLATFORM, name="PROOF-ONLY-receipt.json")), (RELEASE, self.receipt(RELEASE))]
        self.assert_refused(self.run_tool(named), f"{PLATFORM}: receipt: the receipt file is named PROOF-ONLY")
        # A file that is not a receipt, a missing receipt, a link to a receipt.
        (self.root / "lane" / "not-a-receipt.json").write_text('{"made-up": "full report"}\n')
        good = self.receipt(RELEASE)
        (self.root / "lane" / "linked.json").symlink_to(good)
        for path, word in ((self.root / "lane" / "not-a-receipt.json", f"{RELEASE}: receipt: not a receipt"),
                           (self.root / "lane" / "missing.json", f"{RELEASE}: the receipt is not a readable regular file"),
                           (self.root / "lane" / "linked.json", f"{RELEASE}: the receipt is not a readable regular file")):
            self.assert_refused(self.run_tool([(PLATFORM, self.receipt(PLATFORM)), (RELEASE, path)]), word)
        self.assert_nothing_reached_github()

    def test_a_pass_that_is_not_for_exactly_the_rows_is_no_pass(self):
        receipts = [(PLATFORM, self.receipt(PLATFORM, verdict="PASS-FOR-OTHER-ROWS")), (RELEASE, self.receipt(RELEASE))]
        self.assert_refused(self.run_tool(receipts), f"{PLATFORM}: the receipt check's pass is not for exactly these rows")
        self.assert_nothing_reached_github()

    def test_a_pass_that_states_another_commit_is_no_pass(self):
        # A pass of the check states the release commit it judged for and the commit its pins were read
        # from. The tool takes it only when the first is the release commit of this run and the second
        # the commit of the tool's checkout.
        for label, states in (("another release commit", {"release_commit": self.commits["second"]}),
                              ("pins read from another commit", {"pins_commit": self.commits["second"]})):
            with self.subTest(the_pass_states=label):
                receipts = [(PLATFORM, self.receipt(PLATFORM)), (RELEASE, self.receipt(RELEASE, pass_states=states))]
                self.assert_refused(self.run_tool(receipts),
                                    f"{RELEASE}: the receipt check's pass is not for exactly these rows and this commit")
                self.assertEqual([call["target"] for call in self.check_calls()], [PLATFORM, RELEASE])
                self.assert_nothing_reached_github()

    def test_a_file_that_changed_after_its_receipt_refuses(self):
        receipts = self.receipts()
        name = next(iter(PLATFORM_FILES))
        (self.staging / name).write_bytes(PLATFORM_FILES[name] + b"changed after the scans\n")
        self.assert_refused(self.run_tool(receipts), f"custody: {name} is not its row's size and sha256")
        self.assert_nothing_reached_github()
        self.assertEqual(self.check_calls(), [])
        (self.staging / name).write_bytes(PLATFORM_FILES[name].replace(b"product", b"PRODUCT"))    # the same size
        self.assert_refused(self.run_tool(receipts), f"custody: {name} is not its row's size and sha256")
        self.assert_nothing_reached_github()

    def test_the_release_commit_is_the_checkouts_or_the_one_named_and_the_tag_must_select_it(self):
        # Named: receipts of an earlier commit pass the check for that commit, from a checkout at the tip.
        self.head = self.commits["second"]
        receipts = self.receipts()
        release = FakeRelease(self.commits["second"])
        code, lines, _ = self.run_tool(receipts, "--release-commit", self.commits["second"], release=release)
        self.assertEqual(code, 0, lines)
        self.assertIn(f"release_commit={self.commits['second']} tool_commit={self.commits['tip']}", lines[-1])
        self.assertEqual({call["release_commit"] for call in self.check_calls()}, {self.commits["second"]})
        # Not named: the same receipts refuse at the check, for the checkout's commit.
        self.assert_refused(self.run_tool(receipts), "tool_commit is not the release commit")
        # The tag of the release selects another commit than the one the receipts were checked for.
        release = FakeRelease(self.commits["tip"])
        self.assert_refused(self.run_tool(receipts, "--release-commit", self.commits["second"], release=release),
                            "release: release tag does not select accepted source", release=release)
        self.assertEqual(release.downloads, [])


class AttachAndReadBack(Fixture):
    """Steps d and f. Each row is uploaded from the tool's own copy, read back and compared; the tool
    adds to an existing published release and never replaces, deletes or edits anything."""

    def expected(self):
        return {**PLATFORM_FILES, **RELEASE_LEVEL_FILES}

    def stored(self, release=None):
        return {name: asset["data"] for name, asset in (self.release if release is None else release).assets.items()}

    def test_every_row_is_uploaded_from_custody_read_back_and_reported(self):
        staged_before = {path.name: path.read_bytes() for path in self.staging.iterdir()}
        code, lines, error = self.run_tool()
        self.assertEqual((code, error), (0, ""), lines)
        self.assertEqual(self.stored(), self.expected())
        self.assertEqual(self.release.uploads, list(self.expected()))                    # each once, in the receipts' order
        self.assertEqual(sorted(self.release.downloads), sorted(asset["id"] for asset in self.release.assets.values()))
        # The output: one line per row, then the pass line. Names, digests and counts of rows only.
        targets = {**dict.fromkeys(PLATFORM_FILES, PLATFORM), **dict.fromkeys(RELEASE_LEVEL_FILES, RELEASE)}
        self.assertEqual(lines[:-1], [f"attached {targets[name]} {name} {sha256(data)} {len(data)}"
                                      for name, data in self.expected().items()])
        self.assertEqual(lines[-1], f"RELEASE-ATTACH PASS tag={TAG} files=11 attached=11 present=0 "
                                    f"release_commit={self.head} tool_commit={self.head} default_branch=main")
        # The staged files are as they were, and the tool's copies are gone.
        self.assertEqual({path.name: path.read_bytes() for path in self.staging.iterdir()}, staged_before)
        self.assert_no_custody_left()

    def test_the_bytes_that_leave_are_the_checked_copies_not_the_staged_paths(self):
        # The first read of GitHub comes after the check. From then on the staged files are replaced.
        def replace_staged(_endpoint):
            for path in self.staging.iterdir():
                path.write_bytes(b"swapped after the check\n")
        self.release.before_read = replace_staged
        code, lines, _ = self.run_tool()
        self.assertEqual(code, 0, lines)
        self.assertEqual(self.stored(), self.expected())
        # The copy is compared with its row once more as it is read for the upload.
        names = list(self.expected())
        release = FakeRelease(self.head)

        def change_copy(name):
            if name == names[0]:
                (copy,) = self.staging.parent.glob(CUSTODY + "*/files/" + names[1])
                copy.chmod(0o600)
                copy.write_bytes(b"changed in custody\n")
        release.after_upload = change_copy
        for path in self.staging.iterdir():
            path.write_bytes(self.expected()[path.name])
        self.assert_refused(self.run_tool(release=release), f"release: the custody copy of {names[1]} changed before its upload",
                            release=release, written=names[:1])

    def test_an_asset_that_does_not_read_back_as_its_row_refuses(self):
        names = list(self.expected())
        for digests in (True, False):     # with and without a digest in the release's own listing
            with self.subTest(listing_has_digest=digests):
                release = FakeRelease(self.head)
                release.digests, release.stores_other_bytes = digests, names[2]
                result = self.run_tool(release=release)
                self.assert_refused(result, f"release: the asset {names[2]} of the release", release=release, written=names[:3])
                # The rows before it are reported as attached; nothing after it was sent; nothing is deleted.
                self.assertEqual([line.split()[2] for line in result[1] if line.startswith("attached ")], names[:2])
                self.assertIn("attached=2 present=0: 1 reason(s)", result[1][-1])
                self.assertEqual(list(release.assets), names[:3])
                self.assert_no_custody_left()
        # A read-back of the right size and other bytes, where the listing shows no digest.
        release = FakeRelease(self.head)
        release.digests = False
        real = release.download
        release.download = lambda asset_id, stream: stream.write(b"x" * len(self.expected()[names[0]]))
        self.assert_refused(self.run_tool(release=release), f"release: the asset {names[0]} of the release does not read back",
                            release=release, written=names[:1])
        release.download = real

    def test_only_a_complete_asset_counts_at_the_read_back(self):
        names = list(self.expected())
        # A present asset under a row's name, with the row's very bytes, that the release does not show
        # as complete: it is not taken, not read, not replaced and not deleted.
        release = FakeRelease(self.head)
        release.add(names[0], self.expected()[names[0]])
        release.states[names[0]] = "starter"
        self.assert_refused(self.run_tool(release=release), f"release: the asset {names[0]} of the release is not its row's bytes",
                            release=release)
        self.assertEqual((release.downloads, list(release.assets)), ([], [names[0]]))
        # An asset this run uploaded, which the release then shows in another state.
        release = FakeRelease(self.head)
        release.states[names[1]] = "new"
        result = self.run_tool(release=release)
        self.assert_refused(result, f"release: the asset {names[1]} of the release is not its row's bytes", release=release,
                            written=names[:2])
        self.assertIn("attached=1 present=0", result[1][-1])

    def test_an_earlier_asset_that_changes_after_the_last_upload_is_noticed_at_the_end(self):
        names = list(self.expected())

        def replace_first(asset_id):
            last = self.release.assets.get(names[-1])
            if last is not None and asset_id == last["id"]:
                self.release.add(names[0], b"replaced after the last read-back\n")
        self.release.after_download = replace_first
        result = self.run_tool()
        self.assert_refused(result, "release: the assets of the release changed during the read-back", written=names)
        self.assertIn("attached=11 present=0: 1 reason(s)", result[1][-1])

    def test_the_tools_copies_are_private_and_read_only(self):
        seen = {}

        def look(_endpoint):
            if not seen:
                (custody,) = self.staging.parent.glob(CUSTODY + "*")
                seen["directory"] = stat.S_IMODE(custody.stat().st_mode)
                seen["files"] = {path.relative_to(custody).as_posix(): stat.S_IMODE(path.stat().st_mode)
                                 for path in custody.rglob("*") if path.is_file()}
        self.release.before_read = look
        self.assertEqual(self.run_tool()[0], 0)
        # The first read of GitHub comes after the copies were made and checked: eleven files, two receipts.
        self.assertEqual(seen["directory"], 0o700)
        self.assertEqual(sorted(path.split("/")[0] for path in seen["files"]), ["files"] * 11 + ["receipts"] * 2)
        self.assertEqual(set(seen["files"].values()), {0o400})

    def test_a_partial_run_is_completed_by_a_rerun_and_a_complete_release_is_left_alone(self):
        names = list(self.expected())
        receipts = self.receipts()
        self.release.fail_upload = names[3]
        result = self.run_tool(receipts)
        self.assert_refused(result, "release: a request to the release repository failed (OSError)", written=names[:4])
        self.assertIn("attached=3 present=0", result[1][-1])
        self.assertEqual(list(self.release.assets), names[:3])
        # The rerun reads the three present assets back, uploads the other eight and passes.
        self.release.fail_upload, self.release.uploads, self.release.downloads = None, [], []
        code, lines, _ = self.run_tool(receipts)
        self.assertEqual(code, 0, lines)
        self.assertEqual(self.release.uploads, names[3:])
        self.assertEqual([line.split()[0] for line in lines[:-1]], ["present"] * 3 + ["attached"] * 8)
        self.assertIn("files=11 attached=8 present=3", lines[-1])
        self.assertEqual(self.stored(), self.expected())
        self.assertEqual(len(self.release.downloads), 11)       # the present ones were read back too
        # A rerun after full success uploads nothing, reads every asset back and exits 0.
        self.release.uploads, self.release.downloads = [], []
        code, lines, _ = self.run_tool(receipts)
        self.assertEqual((code, self.release.uploads, len(self.release.downloads)), (0, [], 11), lines)
        self.assertIn("files=11 attached=0 present=11", lines[-1])

    def test_a_present_asset_with_a_rows_name_and_other_bytes_refuses_and_nothing_is_written(self):
        names = list(self.expected())
        for label, data in (("another size", b"tampered\n"), ("the same size", b"X" * len(self.expected()[names[4]]))):
            for digests in (True, False):
                with self.subTest(case=label, listing_has_digest=digests):
                    release = FakeRelease(self.head)
                    release.digests = digests
                    release.add(names[4], data)
                    self.assert_refused(self.run_tool(release=release), f"release: the asset {names[4]} of the release", release=release)
                    self.assertEqual({name: asset["data"] for name, asset in release.assets.items()}, {names[4]: data})

    def test_only_an_existing_published_release_of_the_tag_whose_repository_is_public_gets_files(self):
        for label, change, word in (
                ("no release of the tag", {"exists": False}, "release: no published release of the tag"),
                ("a draft", {"draft": True}, "release: no published release of the tag"),
                ("a draft by the time it is read again", {"draft_by_id": True}, "release: the release is a draft"),
                ("a repository that is not public", {"private": True}, "release: selected repository is not public")):
            with self.subTest(case=label):
                release = FakeRelease(self.head)
                for name, value in change.items():
                    setattr(release, name, value)
                self.assert_refused(self.run_tool(release=release), word, release=release)
                self.assertEqual((release.downloads, release.assets), ([], {}))
        # The tag is the release's, in the one form of a Connect release tag.
        for tag in ("v0.0.0", "connect-v0.0", "connect-v0.0.0-rc.1", "meta-v0.0.0", "connect-v0.0.0 "):
            code, _, error = self.run_tool(tag=tag)
            self.assertEqual((code, self.release.reads), (2, []), tag)
            self.assertIn("--tag must be connect-vX.Y.Z", error)

    def test_the_release_is_only_read_and_added_to(self):
        # What the tool asks of GitHub in a full run: reads of the repository, the release, the tag and
        # the asset list, one upload per row and one read-back per row. Nothing else exists to call.
        self.assertEqual(self.run_tool()[0], 0)
        base = "repos/" + candidate.PUBLICATION_REPO
        kinds = {re.sub(r"&page=[0-9]+", "&page=N", endpoint) for endpoint in self.release.reads}
        self.assertEqual(kinds, {base, f"{base}/releases/tags/{TAG}", f"{base}/releases/77", f"{base}/git/ref/tags/{TAG}",
                                 f"{base}/releases/77/assets?per_page=100&page=N"})
        self.assertEqual(sorted(name for name in vars(load("attach_tool_adapter", TOOL).GitHub) if not name.startswith("_")),
                         ["download", "read", "upload"])
        # The tool's own code (docstrings and comments left out) holds no request that edits, deletes
        # or creates, no release command, no other way to the network, and one upload call.
        tree = ast.parse(TOOL.read_text())
        for node in ast.walk(tree):
            if isinstance(node, (ast.Module, ast.ClassDef, ast.FunctionDef)) and ast.get_docstring(node) is not None:
                node.body = node.body[1:] or [ast.Pass()]
        code = ast.unparse(tree)
        for forbidden in ("DELETE", "PATCH", "PUT", "--method", "gh release", "delete-asset", "--clobber", "urllib", "socket",
                          "uploads.github.com", "--draft", "http.client", "requests"):
            self.assertNotIn(forbidden, code, forbidden)
        self.assertEqual(code.count("github.upload("), 1)
        self.assertEqual(code.count("'gh'"), 1)
        self.assertEqual(code.count("https://"), 1)     # the address of the release repository, for the fetch

    def test_the_adapter_reads_uploads_one_new_asset_and_reads_an_asset_back(self):
        # The real adapter, with the two places it starts `gh` replaced: what it would run.
        module = load("attach_tool_real_adapter", TOOL)
        commands = []

        def command_json(argv, *, env=None, stdin=None, **bound):
            commands.append(([str(word) for word in argv], stdin, bound))
            return {"made-up": True}

        def run(argv, **options):
            commands.append((list(argv), options))
            options["stdout"].write(b"made-up asset bytes")

        kept = candidate.command_json
        candidate.command_json = command_json
        module.subprocess = type("Subprocess", (), {"run": staticmethod(run), "DEVNULL": subprocess.DEVNULL})
        try:
            adapter = module.GitHub(candidate)
            self.assertEqual(adapter.read("repos/made-up/endpoint"), {"made-up": True})
            with tempfile.TemporaryFile() as stream:
                adapter.upload(77, "madeup-file.bin", stream)
                self.assertIs(commands[1][1], stream)
            received = io.BytesIO()
            adapter.download(123, received)
        finally:
            candidate.command_json = kept
        self.assertEqual(commands[0], (["gh", "api", "repos/made-up/endpoint", "--hostname", "github.com"], None, {}))
        self.assertEqual(commands[1][0], [
            "gh", "api", f"https://uploads.github.com/repos/{candidate.PUBLICATION_REPO}/releases/77/assets?name=madeup-file.bin",
            "--hostname", "github.com", "--method", "POST", "--header", "Content-Type: application/octet-stream", "--input", "-"])
        self.assertEqual(commands[2][0], ["gh", "api", f"repos/{candidate.PUBLICATION_REPO}/releases/assets/123",
                                          "--hostname", "github.com", "--header", "Accept: application/octet-stream"])
        self.assertEqual(commands[2][1]["stdin"], subprocess.DEVNULL)
        self.assertTrue(commands[2][1]["check"])
        self.assertEqual(received.getvalue(), b"made-up asset bytes")
        self.assertEqual(len(commands), 3)
        # Time bounds. A read or a listing keeps the bound of every `gh` call; the upload of a file
        # and the read-back of an asset, which move a whole archive, have a longer one of their own.
        self.assertEqual(inspect.signature(candidate.command_json).parameters["timeout"].default, 300)
        self.assertGreaterEqual(candidate.UPLOAD_SECONDS, 3600)
        self.assertEqual(commands[1][2], {"timeout": candidate.UPLOAD_SECONDS})
        self.assertEqual(commands[2][1]["timeout"], candidate.UPLOAD_SECONDS)


class NothingElseIsOnTheRelease(Fixture):
    """Step e. At the end the release holds the rows of this run's receipts and, at most, the
    candidate files of publish-accepted. Anything else is named and the run ends non-zero."""

    def test_candidate_files_may_be_on_the_release(self):
        for name in CANDIDATE_FILES:
            self.release.add(name, b"made-up candidate file\n")
        code, lines, _ = self.run_tool()
        self.assertEqual(code, 0, lines)
        self.assertEqual(len(self.release.assets), len(CANDIDATE_FILES) + 11)
        self.assertEqual(len(CANDIDATE_FILES), 3 * len(candidate.TARGETS))

    def test_an_asset_that_is_no_row_and_no_candidate_file_refuses_before_any_write(self):
        self.release.add("madeup-foreign.bin", b"made-up foreign asset\n")
        self.release.add(CANDIDATE_FILES[0], b"made-up candidate file\n")
        self.assert_refused(self.run_tool(), "release: the release holds an asset that is neither a row of a checked receipt nor a "
                            "candidate file: madeup-foreign.bin; nothing was uploaded")
        self.assertEqual(sorted(self.release.assets), sorted(["madeup-foreign.bin", CANDIDATE_FILES[0]]))    # nothing deleted
        # A name that only resembles a candidate file's.
        release = FakeRelease(self.head)
        release.add(CANDIDATE_FILES[0] + ".bak", b"made-up\n")
        self.assert_refused(self.run_tool(release=release), CANDIDATE_FILES[0] + ".bak", release=release)

    def test_an_asset_that_appears_during_the_run_is_named_at_the_end_and_nothing_is_deleted(self):
        names = list({**PLATFORM_FILES, **RELEASE_LEVEL_FILES})

        def late(name):
            if name == names[-1]:
                self.release.add("madeup-late.bin", b"made-up late asset\n")
        self.release.after_upload = late
        result = self.run_tool()
        self.assert_refused(result, "release: the release holds an asset that is neither a row of a receipt this run checked nor "
                            "a candidate file: madeup-late.bin; nothing is deleted", written=names)
        self.assertIn("attached=11 present=0: 1 reason(s)", result[1][-1])
        self.assertEqual(sorted(self.release.assets), sorted([*names, "madeup-late.bin"]))
        # An asset of this run that changes while the run goes on.
        release = FakeRelease(self.head)

        def swap(name):
            if name == names[5]:
                release.add(names[0], b"replaced by another actor\n")
        release.after_upload = swap
        self.assert_refused(self.run_tool(release=release), "release: the assets of the release changed during the run",
                            release=release, written=names[:6])
        # An asset under the name of a row that is still to come: it is not replaced and not uploaded over.
        release = FakeRelease(self.head)

        def early(name):
            if name == names[5]:
                release.add(names[8], b"put there by another actor\n")
        release.after_upload = early
        self.assert_refused(self.run_tool(release=release), f"release: an asset named {names[8]} appeared during the run; "
                            "nothing is replaced", release=release, written=names[:8])
        self.assertEqual(release.assets[names[8]]["data"], b"put there by another actor\n")

    def test_a_later_run_with_other_receipts_ends_non_zero_on_the_earlier_assets(self):
        self.assertEqual(self.run_tool()[0], 0)
        # Another platform's files for the same release, staged with the release's own files and with
        # their two receipts, but without the first platform's files and receipt.
        other = self.root / "later" / "staging"
        other.mkdir(parents=True)
        name = f"madeup-{VERSION}-macos-arm64.tar.gz"
        (other / name).write_bytes(b"made-up archive of another platform\n")
        for present in RELEASE_LEVEL_FILES:
            shutil.copyfile(self.staging / present, other / present)
        receipt = self.root / "later" / "receipt.json"
        receipt.write_text(json.dumps({"schema": "made-up-receipt", "target": "macos-arm64", "version": VERSION, "verdict": "PASS",
                                       "tool_commit": self.head, "payload": self.rows([name], other)}))
        self.release.uploads = []
        result = self.run_tool([("macos-arm64", receipt), (RELEASE, self.receipt(RELEASE))], staging=other)
        self.assert_refused(result, "neither a row of a checked receipt nor a candidate file", next(iter(PLATFORM_FILES)))
        self.assertNotIn(name, self.release.assets)
        # In one run, with all its receipts, the same release takes several platforms.
        both = self.root / "both" / "staging"
        shutil.copytree(self.staging, both)
        shutil.copyfile(other / name, both / name)
        release = FakeRelease(self.head)
        code, lines, _ = self.run_tool([*self.receipts(), ("macos-arm64", receipt)], staging=both, release=release)
        self.assertEqual(code, 0, lines)
        self.assertIn("files=12 attached=12", lines[-1])

    def test_a_run_needs_the_release_receipt_and_a_platform_receipt(self):
        receipts = dict(self.receipts())
        cases = {"the platform alone": ([PLATFORM], PLATFORM_FILES), "the release's own files alone": ([RELEASE], RELEASE_LEVEL_FILES)}
        for label, (targets, names) in cases.items():
            with self.subTest(receipts=label):
                staging = self.root / label.replace(" ", "-") / "staging"
                staging.mkdir(parents=True)
                for name in names:
                    shutil.copyfile(self.staging / name, staging / name)
                self.assert_refused(self.run_tool([(target, receipts[target]) for target in targets], staging=staging),
                                    "receipts: a run needs the receipt of `release` and of at least one platform target")
                self.assert_nothing_reached_github()
                self.assertEqual(self.check_calls(), [])
        # Two platforms without `release` are no release either.
        name = f"madeup-{VERSION}-macos-arm64.tar.gz"
        (self.staging / name).write_bytes(b"made-up archive of another platform\n")
        second = self.receipt(PLATFORM, [name], states_target="macos-arm64")
        self.assert_refused(self.run_tool([(PLATFORM, receipts[PLATFORM]), ("macos-arm64", second)]),
                            "receipts: a run needs the receipt of `release` and of at least one platform target")
        self.assert_nothing_reached_github()


class CommandLine(Fixture):
    def test_usage_errors_stop_before_anything_is_read(self):
        receipts = self.receipts()
        pairs = [f"{target}={path}" for target, path in receipts]
        module = self.tool()
        module.GitHub = lambda _candidate: self.release
        module.committed_sources = lambda: self.fail("the checkout was read")
        for arguments in (["--tag", TAG, "--receipt", pairs[0]],                                        # no staging directory
                          ["--staging", str(self.staging), "--receipt", pairs[0]],                       # no tag
                          ["--staging", str(self.staging), "--tag", TAG],                                # no receipt
                          ["--staging", str(self.staging), "--tag", TAG, "--receipt", str(receipts[0][1])],    # no target
                          ["--staging", str(self.staging), "--tag", TAG, "--receipt", "Linux=" + str(receipts[0][1])],
                          ["--staging", str(self.staging), "--tag", TAG, "--receipt", pairs[0], "--receipt", pairs[0]],
                          ["--staging", str(self.staging / "SHA256SUMS"), "--tag", TAG, "--receipt", pairs[0]],
                          ["--staging", str(self.staging), "--tag", TAG, "--receipt", pairs[0], "--release-commit", "abc"],
                          ["--staging", str(self.staging), "--tag", TAG, "--receipt", pairs[0], "--release-commit", "A" * 40],
                          # No option names a pin, a report, a repository, a branch or a way around a step.
                          *(["--staging", str(self.staging), "--tag", TAG, "--receipt", pairs[0], option, "x"] for option in (
                              "--repository", "--default-branch", "--privacy-report", "--credential-report", "--pins",
                              "--version", "--max-age-hours", "--skip", "--force", "--clobber", "--custody"))):
            out, err = io.StringIO(), io.StringIO()
            with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
                try:
                    code = module.main(arguments)
                except SystemExit as error:
                    code = error.code
            self.assertEqual((code, out.getvalue()), (2, ""), arguments)
        self.assert_nothing_reached_github()

    def test_the_file_runs_as_a_command_in_isolated_mode(self):
        done = subprocess.run([sys.executable, "-I", "-B", str(self.checkout / "connect/scripts" / TOOL.name),
                               "--staging", str(self.staging), "--tag", "not-a-connect-tag", "--receipt", "release=/nowhere"],
                              capture_output=True, text=True)
        self.assertEqual((done.returncode, done.stdout), (2, ""))
        self.assertIn("release-upload-attach: usage error: --tag must be connect-vX.Y.Z", done.stderr)


def stamp():
    return time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime(time.time() - 60))


REAL_PINS = {"name_list_sha256": "a1" * 32, "rules_sha256": "8b" * 32, "non_published_list_sha256": "d5" * 32,
             "detector_pinned_sha256": "b2" * 32, "detector_current_sha256": "c3" * 32}
REAL_EXCEPTIONS = '{"schema": "release-payload-privacy-scan/coverage-exceptions/1", "exceptions": []}\n'
REAL_PINS_FILE = json.dumps({"schema": "release-upload-receipt-check/pins/2", "targets": {
    PLATFORM: {"names": "holds-target", **REAL_PINS},
    RELEASE: {"names": ["RELEASE-MANIFEST.json", "SHA256SUMS", "source-manifest.json",
                        "madeup_toolkit-<version>-py3-none-any.whl", "madeup-toolkit-<version>.tgz"], **REAL_PINS}}}, indent=2) + "\n"


REAL_RECEIPT_SCHEMA = "release-upload-receipt/3"


@unittest.skipUnless(getattr(load("the_repository_check", REAL_CHECK), "RECEIPT_SCHEMA", None) == REAL_RECEIPT_SCHEMA,
                     "the receipt check of this tree does not read receipts of the schema these rows write")
class WithTheRepositorysCheck(Fixture):
    """The tool with the receipt check of this tree, in a made-up checkout whose commit holds made-up
    pins and name rules for a platform target and for `release`."""
    check = REAL_CHECK
    files = (("connect/release/upload-receipt-pins.json", REAL_PINS_FILE),
             ("connect/release/payload-scan-coverage-exceptions.json", REAL_EXCEPTIONS))

    def receipt(self, target, names=None, name=None, **changes):
        names = list(PLATFORM_FILES if target == PLATFORM else RELEASE_LEVEL_FILES) if names is None else names
        scan = {"status": "PASS", "release": True, "generated_at": stamp(), "tool_commit": self.head}
        value = {"schema": REAL_RECEIPT_SCHEMA, "target": target, "payload": self.rows(names),
                 "privacy": {**scan, "tool_sha256": "9a" * 32, "report_sha256": "1b" * 32, "exit_code": 0,
                             "rules_sha256": REAL_PINS["rules_sha256"], "name_list_sha256": REAL_PINS["name_list_sha256"],
                             "non_published_list_sha256": REAL_PINS["non_published_list_sha256"],
                             "coverage_exceptions_sha256": sha256(REAL_EXCEPTIONS.encode()), "blocking_rules": True,
                             "name_rules_all_blocking": True, "no_exemptions": True, "accepted_only_with_proof": True,
                             "accepted_proof_tools": {}, **changes.get("privacy", {})},
                 "credential": {**scan, "tool_sha256": "7c" * 32, "report_sha256": "2c" * 32,
                                "detector_pinned_sha256": REAL_PINS["detector_pinned_sha256"],
                                "detector_current_sha256": REAL_PINS["detector_current_sha256"], "nothing_refused": True,
                                "nothing_pending": True, "nothing_unread_unaccepted": True, **changes.get("credential", {})}}
        self.count += 1
        path = self.root / "lane" / (name or f"receipt-{target}-{self.count}.json")
        path.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n")
        return path

    def test_receipts_that_pass_the_check_attach_every_staged_file(self):
        code, lines, _ = self.run_tool()
        self.assertEqual(code, 0, lines)
        self.assertIn("files=11 attached=11 present=0", lines[-1])
        self.assertEqual({name: asset["data"] for name, asset in self.release.assets.items()},
                         {**PLATFORM_FILES, **RELEASE_LEVEL_FILES})

    def test_a_receipt_the_check_refuses_attaches_nothing(self):
        for label, target, changes in (("a scan that did not pass", RELEASE, {"privacy": {"status": "REFUSED"}}),
                                       ("another detector build", PLATFORM, {"credential": {"detector_current_sha256": "0" * 64}}),
                                       ("a scan with an exemption", RELEASE, {"privacy": {"no_exemptions": False}}),
                                       ("an acceptance without proof", PLATFORM, {"privacy": {"accepted_only_with_proof": False}}),
                                       ("a stale scan", PLATFORM, {"credential": {"generated_at": "2020-01-01T00:00:00Z"}})):
            with self.subTest(case=label):
                receipts = dict(self.receipts())
                receipts[target] = self.receipt(target, **changes)
                self.assert_refused(self.run_tool(list(receipts.items())), target + ": ")
                self.assert_nothing_reached_github()
        # A file that names no platform, under the platform's receipt; a platform file under `release`.
        moved = [(PLATFORM, self.receipt(PLATFORM, [*PLATFORM_FILES, "SHA256SUMS"])),
                 (RELEASE, self.receipt(RELEASE, [name for name in RELEASE_LEVEL_FILES if name != "SHA256SUMS"]))]
        self.assert_refused(self.run_tool(moved), PLATFORM + ": ")
        self.assert_nothing_reached_github()
        # A platform file named for another version than the tag's.
        first = next(iter(PLATFORM_FILES))
        other = first.replace(VERSION, "9.9.9")
        (self.staging / first).rename(self.staging / other)
        renamed = [(PLATFORM, self.receipt(PLATFORM, [other, *list(PLATFORM_FILES)[1:]])), (RELEASE, self.receipt(RELEASE))]
        self.assert_refused(self.run_tool(renamed), PLATFORM + ": ")
        self.assert_nothing_reached_github()

    def test_receipts_of_scans_of_another_commit_attach_nothing(self):
        # The scans ran from an earlier commit of the default branch than the commit being released.
        self.head = self.commits["second"]
        receipts = self.receipts()
        self.head = self.commits["tip"]
        result = self.run_tool(receipts)
        self.assert_refused(result, *(f"{target}: {scan} scan: tool_commit is not the release commit"
                                      for target in (PLATFORM, RELEASE) for scan in ("privacy", "credential")))
        self.assertIn("attached=0 present=0: 4 reason(s)", result[1][-1])
        self.assert_nothing_reached_github()
        # Named as the release commit, for a release whose tag selects it, the same receipts attach.
        release = FakeRelease(self.commits["second"])
        code, lines, _ = self.run_tool(receipts, "--release-commit", self.commits["second"], release=release)
        self.assertEqual(code, 0, lines)
        self.assertIn(f"release_commit={self.commits['second']} tool_commit={self.commits['tip']}", lines[-1])

    def test_the_pins_that_judge_are_those_of_the_commit_the_rule_was_asked_for(self):
        # The check reads the commit of the checkout on its own and takes the pins from it. Here the
        # checkout holds a second commit, on no branch of the release repository, whose pins name
        # another rules file, and the receipts are of a scan with that rules file.
        checkout, tip = self.checkout, self.commits["tip"]
        pins = checkout / "connect/release/upload-receipt-pins.json"
        pins.write_text(pins.read_text().replace(REAL_PINS["rules_sha256"], "77" * 32))
        git(checkout, "add", "-A")
        git(checkout, "commit", "-q", "-m", "pins that no review saw")
        unreviewed = git(checkout, "rev-parse", "HEAD")
        git(checkout, "checkout", "-q", "--detach", tip)

        def receipts():
            return [(target, self.receipt(target, privacy={"rules_sha256": "77" * 32})) for target in (PLATFORM, RELEASE)]

        # Under the reviewed commit's pins they refuse.
        self.assert_refused(self.run_tool(receipts()), "privacy scan: rules_sha256 is not the pinned rules file")
        # The checkout moves to the second commit after the reviewed-commit rule passed for the first:
        # the check would now pass the receipts by the second commit's pins.
        module = self.tool()
        real = module.require_reviewed_commit

        def moved(commit, repository):
            branch = real(commit, repository)
            git(checkout, "checkout", "-q", "--detach", unreviewed)
            return branch

        module.require_reviewed_commit = moved
        self.assert_refused(self.run_tool(receipts(), module=module),
                            f"{PLATFORM}: the receipt check's pass is not for exactly these rows and this commit",
                            f"{RELEASE}: the receipt check's pass is not for exactly these rows and this commit")
        self.assert_nothing_reached_github()
        self.assertEqual(git(checkout, "rev-parse", "HEAD"), unreviewed)


if __name__ == "__main__":
    unittest.main()
