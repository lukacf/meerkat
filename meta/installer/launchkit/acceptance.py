"""Independent, protected semantic acceptance evidence for generated source.

The reviewer owns tests, the host owns execution and receipts, and the builder
owns application source. These are separate authority boundaries. A receipt is
an offline observation and independent judgment, never live deployment proof.
"""
from __future__ import annotations

from contextlib import contextmanager
import fcntl
import hashlib
import hmac
import json
import os
from pathlib import Path
import stat
import sys
import tempfile
import time
import uuid
from .generator import PackageFile
from .dom_check import is_dom_test, run_dom_tests

from .implementation_workspace import (
    ImplementationWorkspace, WorkspaceError, _atomic_write, _check_environment,
    _json, _now, _regular, _read_package_file, _run_sandboxed, _safe_name, _sandbox_executable,
    _sandbox_profile, _sandbox_description, _sha, _trusted_python_paths,
    CHECK_TOTAL_SECONDS, CHECK_TIMEOUT_SECONDS, MAX_FILE_BYTES,
)

MAX_ACCEPTANCE_FILES = 32
MAX_ACCEPTANCE_BYTES = 1024 * 1024
REVIEWER_IDENTITY = "acceptance:main"
POLICY_VERSION = 1
RESULT_MARKER = "MOBKIT_ACCEPTANCE_RESULT="


def tracked_requirements(frozen):
    """Preserve existing outcome IDs; old briefs get one tracking-only outcome."""
    brief = frozen.get("brief") or {}
    rows = [{"id": row["id"], "text": row["outcome"],
             "disposition": row.get("disposition", "open"),
             "supported_subset": row.get("supported_subset", "")}
            for row in brief.get("requirements", [])]
    if not rows:
        text = (brief.get("facts", {}).get("outcome", {}).get("summary")
                or brief.get("mission") or frozen.get("request")
                or "Implement the requested system from the frozen user brief.")
        rows = [{"id": "requested-outcome", "text": text, "disposition": "open", "supported_subset": ""}]
    return rows


class SemanticAcceptance:
    """Host-private tests and append-only review rounds for one frozen build."""

    def __init__(self, root, workspace: ImplementationWorkspace, frozen):
        candidate = Path(root).absolute()
        if candidate.is_symlink():
            raise WorkspaceError("Acceptance storage cannot be a symlink.")
        candidate.mkdir(parents=True, mode=0o700, exist_ok=True)
        self.root = candidate.resolve()
        # Opening a reader leaves a private root's metadata unchanged:
        # recovery observes it, and a same-mode chmod still moves ctime.
        if stat.S_IMODE(self.root.stat().st_mode) != 0o700:
            os.chmod(self.root, 0o700)
        if self.root == workspace.project or workspace.project in self.root.parents:
            raise WorkspaceError("Acceptance tests belong outside the builder's project.")
        self.workspace = workspace
        self.frozen = frozen
        self.blueprint_digest = _sha(_json(frozen["blueprint"]))
        self.brief_digest = _sha(_json(frozen.get("brief") or {}))
        self.requirements = tracked_requirements(frozen)
        binding = {"blueprint": self.blueprint_digest, "brief": self.brief_digest,
                   "messages": frozen.get("messages", []), "request": frozen.get("request", "")}
        requests = frozen.get("implementation_requests", [])
        # A legacy build or a single latest request is already bound above.
        # Additional feedback is part of the independently reviewed requirement
        # history and cannot be changed while reusing earlier acceptance.
        if requests and requests != [binding["request"]]:
            binding["implementation_requests"] = requests
        if frozen.get("organization") is not None:
            binding["organization"] = frozen["organization"]
        self.binding_digest = _sha(_json(binding))
        self.state_path = self.root / "acceptance-state.json"
        self.key_path = self.root / "acceptance-key"
        self.lock_path = self.root / "acceptance.lock"
        for path in (self.state_path, self.key_path, self.lock_path):
            if path.is_symlink():
                raise WorkspaceError("Acceptance storage cannot contain links.")
        with self._locked():
            if not self.state_path.exists():
                _atomic_write(self.key_path, os.urandom(32).hex() + "\n")
                self._save({"binding_digest": self.binding_digest, "rounds": []})
            self._state()

    @contextmanager
    def _locked(self):
        descriptor = os.open(self.lock_path, os.O_CREAT | os.O_RDWR | os.O_NOFOLLOW, 0o600)
        try:
            if os.fstat(descriptor).st_nlink != 1:
                raise WorkspaceError("Acceptance lock cannot be a hard link.")
            fcntl.flock(descriptor, fcntl.LOCK_EX)
            yield
        finally:
            os.close(descriptor)

    def _state(self):
        _regular(self.state_path)
        value = json.loads(self.state_path.read_text())
        if value.get("binding_digest") != self.binding_digest:
            raise WorkspaceError("Acceptance evidence belongs to a different frozen brief or design.")
        return value

    def _save(self, state):
        _atomic_write(self.state_path, _json(state) + "\n")

    def _signed(self, value):
        _regular(self.key_path)
        key = bytes.fromhex(self.key_path.read_text().strip())
        return hmac.new(key, _json({k: v for k, v in value.items() if k != "signature"}).encode(), hashlib.sha256).hexdigest()

    def _round(self, state):
        if not state["rounds"]:
            raise WorkspaceError("Start a native reviewer round first.")
        return state["rounds"][-1]

    def begin_round(self):
        """Freeze source and retain every earlier test, result and native run."""
        files = self.workspace.export_files(require_verified=False)
        source_digest = self.workspace._digest(files)
        with self._locked():
            state = self._state()
            number = len(state["rounds"]) + 1
            directory = self.root / f"round-{number}"
            directory.mkdir(mode=0o700)
            source = directory / "source"
            tests = directory / "tests"
            source.mkdir(mode=0o700)
            tests.mkdir(mode=0o700)
            for item in files:
                path = source / item.path
                path.parent.mkdir(parents=True, mode=0o700, exist_ok=True)
                _atomic_write(path, item.content, item.mode)
            if state["rounds"]:
                for name, content in self._tests(state["rounds"][-1]).items():
                    _atomic_write(tests / name, content)
            row = {"number": number, "source_digest": source_digest, "started_at": _now(),
                   "reviewer_identity": REVIEWER_IDENTITY, "reviewer_interaction_id": None,
                   "execution": None, "publication": None, "native_completion": None}
            state["rounds"].append(row)
            self._save(state)
            return json.loads(_json(row))

    def bind_run(self, interaction_id):
        if not isinstance(interaction_id, str) or not interaction_id:
            raise WorkspaceError("Bind the admitted native reviewer interaction.")
        with self._locked():
            state = self._state()
            row = self._round(state)
            if row["reviewer_interaction_id"] not in (None, interaction_id):
                raise WorkspaceError("This reviewer round already has a native interaction.")
            row["reviewer_interaction_id"] = interaction_id
            self._save(state)

    def _tests(self, row):
        directory = self.root / f"round-{row['number']}" / "tests"
        if directory.is_symlink() or not directory.is_dir():
            raise WorkspaceError("Protected acceptance tests cannot be links.")
        result = {}
        for path in sorted(directory.iterdir()):
            if path.is_symlink() or not path.name.startswith("test_") or not (path.suffix == ".py" or is_dom_test(path.name)):
                raise WorkspaceError("Acceptance tests must be ordinary test_*.py or test_*.dom.cjs files.")
            info = _regular(path)
            if info.st_size > MAX_FILE_BYTES:
                raise WorkspaceError("An acceptance test exceeds its size limit.")
            result[path.name] = path.read_text(encoding="utf-8")
        if len(result) > MAX_ACCEPTANCE_FILES or sum(len(v.encode()) for v in result.values()) > MAX_ACCEPTANCE_BYTES:
            raise WorkspaceError("Acceptance tests exceed their size limit.")
        return result

    def context(self):
        with self._locked():
            state = self._state()
            row = self._round(state)
            source = self.root / f"round-{row['number']}" / "source"
            files = []
            for path in sorted(source.rglob("*")):
                if path.is_file():
                    name = _safe_name(path.relative_to(source).as_posix())
                    item = _read_package_file(path, name)
                    files.append({"path": name, "sha256": _sha(item.content),
                                  **({"binary": True, "size": len(item.content)}
                                     if isinstance(item.content, bytes) else {})})
            return {**({"organization": json.loads(_json(self.frozen["organization"]))}
                       if self.frozen.get("organization") is not None else {}),
                    "brief": self.frozen.get("brief", {}), "blueprint": self.frozen["blueprint"],
                    "messages": self.frozen.get("messages", []), "request": self.frozen.get("request", ""),
                    "implementation_requests": self.frozen.get("implementation_requests", []),
                    "requirements": self.requirements, "source_digest": row["source_digest"], "review_round": row["number"],
                    "files": files, "acceptance_tests": list(self._tests(row)),
                    "prior_reviews": [value.get("publication") for value in state["rounds"][:-1]],
                    "scope": "Independent offline semantic review. Live transport, credentials and deployment are not proven."}

    def _source_digest(self, row):
        source = self.root / f"round-{row['number']}" / "source"
        if source.is_symlink() or not source.is_dir():
            raise WorkspaceError("Frozen source must remain an ordinary directory.")
        files = []
        for directory, children, names in os.walk(source, followlinks=False):
            if any((Path(directory) / name).is_symlink() for name in children):
                raise WorkspaceError("Frozen source directories cannot be links.")
            for name in names:
                path = Path(directory) / name
                relative = _safe_name(path.relative_to(source).as_posix())
                files.append(_read_package_file(path, relative))
        return self.workspace._digest(sorted(files, key=lambda item: item.path))

    def read_source(self, path):
        path = _safe_name(path)
        if path.lower().endswith(".whl"):
            raise WorkspaceError("Host-adopted wheels expose metadata only, not text content.")
        with self._locked():
            row = self._round(self._state())
            root = self.root / f"round-{row['number']}" / "source"
            target = root / path
            if any(parent.is_symlink() for parent in target.parents if parent != self.root):
                raise WorkspaceError("Frozen source cannot contain links.")
            _regular(target)
            return target.read_text(encoding="utf-8")

    def write_test(self, path, content):
        path = _safe_name(path)
        if "/" in path or not path.startswith("test_") or not (path.endswith(".py") or is_dom_test(path)):
            raise WorkspaceError("Use a flat test_*.py or test_*.dom.cjs acceptance test filename.")
        if not isinstance(content, str) or "\x00" in content or len(content.encode()) > MAX_ACCEPTANCE_BYTES:
            raise WorkspaceError("Write bounded UTF-8 acceptance test source.")
        if path.endswith(".py"):
            compile(content, path, "exec", dont_inherit=True)
        with self._locked():
            state = self._state()
            row = self._round(state)
            if row["publication"]:
                raise WorkspaceError("Published acceptance rounds are sealed. Start the next host review round.")
            tests = self._tests(row)
            tests[path] = content
            if len(tests) > MAX_ACCEPTANCE_FILES or sum(len(v.encode()) for v in tests.values()) > MAX_ACCEPTANCE_BYTES:
                raise WorkspaceError("Acceptance tests exceed their size limit.")
            target = self.root / f"round-{row['number']}" / "tests" / path
            if target.exists():
                _regular(target)
            _atomic_write(target, content)
            row["execution"] = None
            self._save(state)
            return {"path": path, "sha256": _sha(content)}

    def read_test(self, path):
        with self._locked():
            return self._tests(self._round(self._state()))[_safe_name(path)]

    def check(self):
        """Execute only independently authored protected tests on frozen source."""
        with self._locked():
            state = self._state()
            row = self._round(state)
            if row["publication"]:
                raise WorkspaceError("Published acceptance rounds are sealed.")
            row["execution"] = None
            self._save(state)
            tests = self._tests(row)
            test_digest = _sha(_json(tests))
            if self.workspace.digest() != row["source_digest"] or self._source_digest(row) != row["source_digest"]:
                raise WorkspaceError("Source changed after this review snapshot. Start a fresh review.")
            result = {"status": "fail", "detail": "The reviewer must author meaningful executable acceptance tests.",
                      "tests_run": 0, "test_results": []}
            if tests:
                with tempfile.TemporaryDirectory(prefix="mobkit-acceptance-") as temporary:
                    staging = Path(temporary).resolve()
                    source = self.root / f"round-{row['number']}" / "source"
                    snapshot = staging / "project"
                    import shutil
                    shutil.copytree(source, snapshot)
                    protected = staging / "acceptance-tests"
                    protected.mkdir(mode=0o700)
                    for name, content in tests.items():
                        _atomic_write(protected / name, content)
                    scratch = staging / "scratch"
                    scratch.mkdir(mode=0o700)
                    runner = staging / "host-check.py"
                    runner.write_text(_ACCEPTANCE_HARNESS)
                    token = uuid.uuid4().hex
                    python_tests = [name for name in tests if name.endswith(".py")]
                    dom_tests = [name for name in tests if is_dom_test(name)]
                    deadline = time.monotonic() + CHECK_TOTAL_SECONDS
                    results = []
                    if python_tests:
                        command = [str(Path(sys.executable).resolve()), "-I", "-S", str(runner),
                                   str(snapshot), _json(_trusted_python_paths()), str(protected), token, *python_tests]
                        raw = _run_sandboxed(command, _sandbox_executable(),
                            _sandbox_profile(snapshot, scratch, staging, protected_reads=[protected]),
                            snapshot, _check_environment(scratch), timeout=CHECK_TOTAL_SECONDS)
                        results.append(_execution_result(raw, token, set(python_tests)))
                    if dom_tests:
                        def execute_dom(command, read_paths, input_bytes=None):
                            remaining = deadline - time.monotonic()
                            if remaining <= 0:
                                return {"status": "fail", "detail": "Acceptance exceeded its total executable time limit."}
                            return _run_sandboxed(command, _sandbox_executable(),
                                _sandbox_profile(snapshot, scratch, staging, protected_reads=read_paths),
                                snapshot, _check_environment(scratch), timeout=min(CHECK_TIMEOUT_SECONDS, remaining),
                                **({"input_bytes": input_bytes} if input_bytes is not None else {}))
                        results.append(run_dom_tests(dom_tests, snapshot, protected, scratch, staging, execute_dom))
                    statuses = {value["status"] for value in results}
                    result = {"status": "fail" if "fail" in statuses else "unavailable" if "unavailable" in statuses else "pass",
                              "tests_run": sum(value["tests_run"] for value in results),
                              "test_results": [item for value in results for item in value["test_results"]],
                              "detail": "\n".join(value["detail"] for value in results)}
            execution = {"id": uuid.uuid4().hex, "issued_at": _now(), "source_digest": row["source_digest"],
                         "test_digest": test_digest, "selected_tests": list(tests), **result,
                         "sandbox": _sandbox_description()}
            execution["signature"] = self._signed(execution)
            row["execution"] = execution
            self._save(state)
            return json.loads(_json(execution))

    def publish(self, report):
        """Bind reviewer judgment to actual host results and exact outcome IDs."""
        with self._locked():
            state = self._state()
            row = self._round(state)
            execution = row.get("execution")
            if not row.get("reviewer_interaction_id"):
                raise WorkspaceError("A native reviewer admission is required.")
            if not execution or not hmac.compare_digest(str(execution.get("signature", "")), self._signed(execution)):
                raise WorkspaceError("Run host acceptance tests before publishing an assessment.")
            if (execution["source_digest"] != self.workspace.digest()
                    or execution["test_digest"] != _sha(_json(self._tests(row)))):
                raise WorkspaceError("Source or acceptance tests changed. Run a fresh bound review.")
            requirements = report.get("requirements")
            expected = {value["id"]: value for value in self.requirements}
            if not isinstance(requirements, list) or len(requirements) != len(expected) or {r.get("id") for r in requirements if isinstance(r, dict)} != set(expected):
                raise WorkspaceError("Assess every existing requested outcome exactly once, using its existing ID.")
            passing = {r["id"] for r in execution["test_results"] if r["status"] == "pass"}
            source = self.root / f"round-{row['number']}" / "source"
            assessed = []
            for item in requirements:
                status = item.get("status")
                if status not in ("proposal", "source", "tested", "unresolved"):
                    raise WorkspaceError("Outcome evidence status is proposal, source, tested or unresolved.")
                paths = _strings(item.get("source_paths", []), "source paths")
                for path in paths:
                    _safe_name(path)
                    if not (source / path).is_file():
                        raise WorkspaceError("Source evidence must cite an existing frozen project file.")
                test_ids = _strings(item.get("test_ids", []), "test IDs")
                if set(test_ids) - passing or (status == "tested" and not test_ids):
                    raise WorkspaceError("Tested outcomes need exact passing host-executed test IDs.")
                dependencies = _strings(item.get("remaining_dependencies", []), "remaining dependencies")
                evidence = item.get("evidence", "")
                if not isinstance(evidence, str) or not evidence.strip() or len(evidence) > 4000:
                    raise WorkspaceError("Explain the actual evidence and its limits for each outcome.")
                if status == "source" and not paths:
                    raise WorkspaceError("Source evidence needs an actual source path.")
                if status == "unresolved" and not dependencies:
                    raise WorkspaceError("Unresolved outcomes need concrete remaining dependencies.")
                assessed.append({**expected[item["id"]], "status": status, "source_paths": paths,
                                 "test_ids": test_ids, "evidence": evidence, "remaining_dependencies": dependencies})
            counterexamples = _strings(report.get("counterexamples", []), "counterexamples")
            dependencies = _strings(report.get("remaining_dependencies", []), "remaining dependencies")
            summary = report.get("summary", "")
            if not isinstance(summary, str) or not summary.strip() or len(summary) > 6000:
                raise WorkspaceError("Provide a concise independent review summary.")
            complete = (execution["status"] == "pass" and execution["tests_run"] > 0
                        and self._has_active_tested_outcome(assessed, execution))
            status = "pass" if complete and not counterexamples else "needs_repair" if counterexamples or execution["status"] == "fail" else "incomplete"
            if any(r["status"] in ("proposal", "unresolved") and r["disposition"] not in ("deferred", "cancelled") for r in assessed):
                status = "needs_repair"
            receipt = {"id": uuid.uuid4().hex, "issued_at": _now(), "issuer": "mobkit-launch-acceptance-host",
                       "acceptance_policy_version": POLICY_VERSION,
                       "status": status, "source_digest": row["source_digest"], "blueprint_digest": self.blueprint_digest,
                       "brief_digest": self.brief_digest, "binding_digest": self.binding_digest,
                       "review_round": row["number"], "reviewer_identity": REVIEWER_IDENTITY,
                       "reviewer_interaction_id": row["reviewer_interaction_id"], "summary": summary,
                       "requirements": assessed, "counterexamples": counterexamples,
                       "remaining_dependencies": dependencies, "tests_run": execution["tests_run"],
                       "test_digest": execution["test_digest"], "test_results": execution["test_results"],
                       "execution": execution, "native_completion": None,
                       "ready_to_package": False,
                       "scope": "Independent reviewer judgment and protected offline acceptance tests on exact source. Not live-ready; no transport, credential, deployment or hardware proof."}
            receipt["signature"] = self._signed(receipt)
            row["publication"] = receipt
            self._save(state)
            return json.loads(_json(receipt))

    def _has_active_tested_outcome(self, assessed, execution):
        # Use the original brief's dispositions, including for retained receipts.
        active = {row["id"] for row in self.requirements
                  if row["disposition"] not in ("deferred", "cancelled")}
        if not active:
            return True
        passing = {row["id"] for row in execution.get("test_results", []) if row["status"] == "pass"}
        return any(row["id"] in active and row["status"] == "tested"
                   and row.get("test_ids") and not set(row["test_ids"]) - passing for row in assessed)

    def complete_run(self, interaction_id, completion):
        """Host seals the actual typed native completion after reviewer publication.

        This method is deliberately absent from reviewer and builder tool grants.
        A published assessment followed by a native failure is never accepted.
        """
        if (not isinstance(completion, dict) or completion.get("kind") != "interaction_complete"
                or completion.get("interaction_id") != interaction_id or not completion.get("cursor")):
            raise WorkspaceError("Seal acceptance only with an observed typed native interaction completion.")
        with self._locked():
            state = self._state()
            row = self._round(state)
            receipt = row.get("publication")
            if row.get("reviewer_interaction_id") != interaction_id or not receipt:
                raise WorkspaceError("The completed native run must match this published reviewer assessment.")
            if not hmac.compare_digest(str(receipt.get("signature", "")), self._signed(receipt)):
                raise WorkspaceError("The provisional acceptance assessment was altered.")
            if (receipt["source_digest"] != self.workspace.digest()
                    or self._source_digest(row) != receipt["source_digest"]
                    or receipt["test_digest"] != _sha(_json(self._tests(row)))):
                raise WorkspaceError("Source or tests changed before the native reviewer completed.")
            row["native_completion"] = {"kind": completion["kind"], "interaction_id": interaction_id,
                                        "cursor": completion["cursor"], "observed_at": _now()}
            receipt["native_completion"] = row["native_completion"]
            receipt["ready_to_package"] = (receipt["status"] == "pass"
                and self._has_active_tested_outcome(receipt["requirements"], receipt["execution"]))
            receipt["signature"] = self._signed(receipt)
            self._save(state)
            return json.loads(_json(receipt))

    def verification(self):
        """Return only current authentic evidence, never a model's receipt claim."""
        with self._locked():
            state = self._state()
            if not state["rounds"]:
                return None
            row = self._round(state)
            receipt = row.get("publication")
            if not receipt or not receipt.get("native_completion") or receipt.get("source_digest") != self.workspace.digest():
                return None
            if receipt.get("test_digest") != _sha(_json(self._tests(row))):
                return None
            if self._source_digest(row) != receipt.get("source_digest"):
                return None
            if not hmac.compare_digest(str(receipt.get("signature", "")), self._signed(receipt)):
                return None
            if (receipt["status"] == "pass"
                    and not self._has_active_tested_outcome(receipt["requirements"], receipt["execution"])):
                return None
            return json.loads(_json(receipt))

    def export_files(self):
        receipt = self.verification()
        if not receipt or not receipt.get("ready_to_package"):
            raise WorkspaceError("Packaging requires current independently accepted offline source.")
        with self._locked():
            row = self._round(self._state())
            return [PackageFile("acceptance-report.json", _json(receipt) + "\n"),
                    *[PackageFile("acceptance-tests/" + name, content) for name, content in self._tests(row).items()]]

    def history(self):
        with self._locked():
            return json.loads(_json(self._state()["rounds"]))


def _strings(value, name):
    if not isinstance(value, list) or len(value) > 100 or any(not isinstance(v, str) or not v.strip() or len(v) > 4000 for v in value):
        raise WorkspaceError(f"Provide a bounded list of nonempty {name}.")
    return value


def _execution_result(raw, token, test_files):
    result = {**raw, "tests_run": 0, "test_results": []}
    try:
        marker = RESULT_MARKER + token + "="
        # One host completion only. Fixed strings printed by source do not count.
        matches = [line[len(marker):] for line in raw.get("detail", "").splitlines() if line.startswith(marker)]
        if len(matches) != 1:
            raise ValueError("missing or duplicate host completion")
        value = json.loads(matches[0])
        rows = value["test_results"]
        if (type(value["tests_run"]) is not int or value["tests_run"] <= 0 or not isinstance(rows, list)
                or len(rows) != value["tests_run"] or len({r["id"] for r in rows}) != len(rows)
                or any(r["file"] not in test_files or r["status"] not in ("pass", "fail", "error", "skipped") for r in rows)
                or not any(r["status"] != "skipped" for r in rows)):
            raise ValueError("empty, skipped or invalid test evidence")
        result.update(tests_run=value["tests_run"], test_results=rows)
        if any(r["status"] in ("fail", "error") for r in rows):
            result["status"] = "fail"
        result["detail"] = raw["detail"].replace(marker + matches[0], "Host acceptance harness completed.")
    except (KeyError, ValueError, TypeError):
        if raw["status"] != "unavailable":
            result["status"] = "fail"
        result["detail"] = "No valid nonempty protected acceptance completion.\n" + raw.get("detail", "")
    return result


_ACCEPTANCE_HARNESS = r'''import importlib.util, json, pathlib, sys, traceback, unittest
root = pathlib.Path(sys.argv[1]).resolve()
sys.path[:0] = [str(root), *json.loads(sys.argv[2])]
protected = pathlib.Path(sys.argv[3]).resolve()
token = sys.argv[4]
paths = sys.argv[5:]
sys.argv = [str(root)]
class Results(unittest.TextTestResult):
    def __init__(self, *args):
        super().__init__(*args)
        self.observed = []
    def record(self, test, status, detail=""):
        filename = test.__class__.__module__.split("protected_acceptance_", 1)[-1] + ".py"
        identity = filename + "::" + test.__class__.__qualname__ + "." + test._testMethodName
        existing = next((row for row in self.observed if row["id"] == identity), None)
        if existing:
            if {"skipped":0, "pass":1, "fail":2, "error":3}[status] > {"skipped":0, "pass":1, "fail":2, "error":3}[existing["status"]]:
                existing["status"] = status
            existing["detail"] = (existing["detail"] + "\n" + detail)[:4000]
        else:
            self.observed.append({"id": identity, "file": filename, "status": status, "detail": detail[:4000]})
    def addSuccess(self, test):
        super().addSuccess(test); self.record(test, "pass")
    def addFailure(self, test, error):
        super().addFailure(test, error); self.record(test, "fail", self._exc_info_to_string(error, test))
    def addError(self, test, error):
        super().addError(test, error); self.record(test, "error", self._exc_info_to_string(error, test))
    def addSkip(self, test, reason):
        super().addSkip(test, reason); self.record(test, "skipped", reason)
    def addExpectedFailure(self, test, error):
        super().addExpectedFailure(test, error); self.record(test, "skipped", "Expected failure is not passing evidence.")
    def addUnexpectedSuccess(self, test):
        super().addUnexpectedSuccess(test); self.record(test, "fail", "Unexpected success violates the acceptance test's expectation.")
    def addSubTest(self, test, subtest, error):
        super().addSubTest(test, subtest, error)
        if error is not None:
            self.record(test, "fail", self._exc_info_to_string(error, test))
suite = unittest.TestSuite()
try:
    for relative in paths:
        path = protected / relative
        name = "protected_acceptance_" + path.stem
        spec = importlib.util.spec_from_file_location(name, path)
        module = importlib.util.module_from_spec(spec)
        sys.modules[name] = module
        spec.loader.exec_module(module)
        suite.addTests(unittest.defaultTestLoader.loadTestsFromModule(module))
    result = unittest.TextTestRunner(verbosity=2, resultclass=Results).run(suite)
except BaseException:
    traceback.print_exc()
    raise SystemExit(1)
print("MOBKIT_ACCEPTANCE_RESULT=" + token + "=" + json.dumps({"tests_run": result.testsRun, "test_results": result.observed}), flush=True)
raise SystemExit(0 if result.wasSuccessful() and result.testsRun > len(result.skipped) else 1)
'''
