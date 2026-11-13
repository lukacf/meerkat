#!/usr/bin/env python3
"""Scan release payload files for credential patterns before upload. The scan fails closed.

  python3 -I release-payload-credential-scan.py --name NAME \\
      --gitleaks-pinned /ABSOLUTE/gitleaks --gitleaks-current /ABSOLUTE/gitleaks \\
      [--default-config-pinned TOML] [--default-config-current TOML] \\
      [--pinned-version LABEL] [--current-version LABEL] \\
      [--accepted-findings ACCEPTED] [--pending PENDING] \\
      --receipt RECEIPT --report REPORT [--unaccepted LISTING] [--work DIR] PAYLOAD...

What it does
  1. Deep extraction of every payload file (release-payload-deep-extract.py beside this file):
     every nested archive, at every level. An archive that cannot be opened refuses the run
     unless the acceptance file lists its exact bytes.
  2. Two builds of the gitleaks detector, a pinned older one and a current one, each given by
     absolute path, read the extracted tree with their own default rules minus the global path
     allowlist lines that mention "vendor", "dist-info" or "lock" (lock files). Each build's
     default rules file is
     gitleaks.toml beside the build, or the file given with --default-config-...; a run with the
     build's embedded default must give the same result, or the scan stops. The builds always run
     with --redact and --ignore-gitleaks-allow, from an empty working directory, with the payload
     one level below the scan root: no comment, ignore file or rules file inside a payload can
     switch a finding off.
  3. Compiled files (ELF) are covered by their printable runs (8 or more printable ASCII bytes).
     Every run a finding touches is scanned again as a unit of its own, and only those findings
     count, so no rule can join the end of one run to the start of the next. Such a finding is
     reported at PATH#strings with the byte offset of the run as its line.
  4. Every finding is a position: rule, path inside the payload ("!" at archive boundaries), line,
     and the sha256 of the member file that holds it. A position without an acceptance bound to
     exactly those bytes REFUSES.

Acceptance file (--accepted-findings, optional and private; without it nothing is accepted),
schema release-payload-credential-scan/accepted/1:
  positions: [{rule, path, sha256, line, class, reason, accepted_by, count?}]
             sha256 is the member file's; line is the line, or the run's byte offset for a
             PATH#strings position; count (default 1) is the number of findings there.
  archives:  [{path, sha256, index (pypi|crates.io|npm), class "PUBLIC-UPSTREAM-ARCHIVE", reason,
               accepted_by}]   everything inside that exact archive, proved byte-equal to the
             public upstream archive.
  files:     [{path, sha256, class "PUBLIC-UPSTREAM-FILE", reason, accepted_by}]   that exact
             file, proved byte-equal to a public upstream file: its findings, and the file itself
             when no build reads it.
  unopened:  [{path, sha256, reason, accepted_by}]   an archive that cannot be opened.
  Every entry needs a non-empty reason and a non-empty accepted_by; the tool has no approver of
  its own, and an entry without one is a usage error. An entry may carry a proof object (what
  was proved and how); the tool keeps it with the entry and does not read it.
Pending list (--pending, optional), schema release-payload-credential-scan/pending/1:
  positions: [{rule, path, line, sha256, note?, count?}]
  It NEVER accepts. A position on it refuses even when an acceptance entry matches, and the
  receipt marks it "pending owner".

Unread rule, per build: a file may be unread by a detector build only when it is byte-equal to a
pinned upstream member; anything else unread REFUSES. A file that starts with a binary signature
and is not an opened archive or an ELF file, a file that a build's remaining path allowlist
skips (both builds, or one build while the other reads it), an archive that could not be opened,
and an archive member that was not written out (an encrypted, unreadable or unsafe-path member)
must each be covered by an acceptance entry for those exact bytes: a files entry, an unopened
entry, or an archives entry for an enclosing archive.

Receipt (--receipt, a new file; its first line is also printed):
  CREDENTIAL-LEG PASS|REFUSED <name> files=<n> <file>=<sha256> ... accepted=<sha256|none>
    pending=<sha256|none> builds=<version>,<version> build_sha256=<sha256>,<sha256> findings=<n>
    accepted_positions=<n> accepted_in_archives=<n> accepted_in_files=<n> refused=<n>
    pending_owner=<n> unread=<n> unread_unaccepted=<n>
PASS only when refused, pending_owner and unread_unaccepted are all 0.
Report (--report, a new file, JSON, schema release-payload-credential-scan/report/1): what an
upload check reads. status "PASS" or "REFUSED"; release (false for a --proof-only run, whose name
and label start with PROOF-ONLY-) and label; generated_at (UTC); tool_sha256; tool_commit (the commit of
the checkout this tool ran from, null when its files are not that commit's); payload [{name,
bytes, sha256}] of the bytes that were scanned; detector_builds [{role, version, sha256}];
accepted and pending {count, sha256}; the counts. The receipt and the report
hold counts, rule ids, paths and positions only. The listing
(--unaccepted, optional, a new file, mode 0600) names every refused position and unread file with
its sha256, which is what an acceptance entry needs. No matched value, no part of one and no line
content is printed, logged or written to any of them. The detectors' redacted reports stay in the
private work directory, which is removed at the end unless --keep-work is given.

Exit status: 0 PASS, 1 REFUSED, 2 usage error, 3 tool failure (also a refusal).
Python 3.11 or later, standard library only, no network.
"""
import argparse
import collections
import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import time
import tomllib
import traceback
import types

HERE = os.path.dirname(os.path.abspath(__file__))
EXTRACTOR = os.path.join(HERE, "release-payload-deep-extract.py")
SCHEMA_ACCEPTED = "release-payload-credential-scan/accepted/1"
SCHEMA_PENDING = "release-payload-credential-scan/pending/1"
SCHEMA_REPORT = "release-payload-credential-scan/report/1"
SCHEMA_UNACCEPTED = "release-payload-credential-scan/unaccepted/1"
HEX64 = re.compile(r"^[0-9a-f]{64}$")
LEAK_EXIT = 42
# A file that starts with one of these is not text; the builds do not read it.
BIN_MAGICS = [(0, b"\x7fELF"), (0, b"%PDF"), (0, b"SQLite format 3\x00"), (0, b"\x00asm"), (0, b"MZ"),
              (0, b"\xca\xfe\xba\xbe"), (0, b"\xcf\xfa\xed\xfe"), (0, b"wOFF"), (0, b"wOF2"),
              (0, b"PK\x03\x04"), (0, b"\x1f\x8b"), (0, b"BZh"), (0, b"\xfd7zXZ\x00"), (0, b"\x28\xb5\x2f\xfd"),
              (257, b"ustar"), (0, b"7z\xbc\xaf\x27\x1c"), (0, b"Rar!"), (0, b"!<arch>\n")]
STRINGS_RX = re.compile(rb"[\x20-\x7e]{8,}")
STRINGS = "#strings"
PROOF_ONLY = "PROOF-ONLY"
# Reasons a member was not written out that leave no content unread.
NO_CONTENT_SKIPS = ("link", "special", "duplicate-name-renamed")
UNOPENED_RULE = "unopened-archive"


class Usage(Exception):
    pass


class Failure(Exception):
    pass


def sha256_file(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def need(cond, msg):
    if not cond:
        raise Usage(msg)


def tool_commit():
    """The commit of the checkout this tool runs from, when this file and the extractor are tracked
    and unchanged there; otherwise None, and no upload check accepts the report. The checkout is
    found from this file's directory: git variables of the caller's environment are not passed on,
    and git is told to read the commit's own objects, not objects a replace ref puts in their place."""
    environment = {key: value for key, value in os.environ.items() if not key.startswith("GIT_")}

    def git(*args):
        try:
            done = subprocess.run(["git", "--no-replace-objects", "-C", HERE, *args], stdout=subprocess.PIPE,
                                  stderr=subprocess.DEVNULL, stdin=subprocess.DEVNULL, env=environment, check=False)
        except OSError:
            return 1, ""
        return done.returncode, done.stdout.decode("utf-8", "replace").strip()
    code, head = git("rev-parse", "--verify", "HEAD^{commit}")
    files = [os.path.abspath(__file__), EXTRACTOR]
    if code != 0 or not re.fullmatch(r"[0-9a-f]{40}([0-9a-f]{24})?", head):
        return None
    if git("ls-files", "--error-unmatch", "--", *files)[0] != 0 or git("diff", "--quiet", "HEAD", "--", *files)[0] != 0:
        return None
    return head


def load_json(path, what):
    """The document and the sha256 of the bytes it was read from."""
    try:
        with open(path, "rb") as f:
            raw = f.read()
        return json.loads(raw), hashlib.sha256(raw).hexdigest()
    except (OSError, ValueError) as exc:
        raise Usage("%s is not readable JSON (%s)" % (what, type(exc).__name__))


def entry(e, where, keys, texts):
    need(isinstance(e, dict) and set(e) <= keys | {"proof"}, where + ": unknown or missing structure")
    need(isinstance(e.get("proof", {}), dict), where + ": proof must be an object")
    for k in texts:
        need(isinstance(e.get(k), str) and e[k].strip(), where + ": %s must be a non-empty string" % k)
    need(isinstance(e.get("sha256"), str) and HEX64.match(e["sha256"]), where + ": sha256 must be 64 lower-hex")


def positive(value):
    return isinstance(value, int) and not isinstance(value, bool) and value >= 1


def validate_accepted(doc):
    need(isinstance(doc, dict) and doc.get("schema") == SCHEMA_ACCEPTED, "acceptance file: schema must be %s" % SCHEMA_ACCEPTED)
    need(set(doc) <= {"schema", "positions", "archives", "files", "unopened", "note"}, "acceptance file: unknown top-level key")
    pos, arch, files, unop = (doc.get(k, []) for k in ("positions", "archives", "files", "unopened"))
    need(all(isinstance(x, list) for x in (pos, arch, files, unop)), "acceptance file: positions, archives, files and unopened must be lists")
    for n, e in enumerate(pos):
        w = "acceptance positions[%d]" % n
        entry(e, w, {"rule", "path", "sha256", "line", "class", "reason", "accepted_by", "count"},
              ("rule", "path", "class", "reason", "accepted_by"))
        need(isinstance(e.get("line"), int) and not isinstance(e.get("line"), bool) and e["line"] >= 0, w + ": line must be an integer")
        need(positive(e.get("count", 1)), w + ": count must be a positive integer")
    for n, e in enumerate(arch):
        w = "acceptance archives[%d]" % n
        entry(e, w, {"path", "sha256", "index", "class", "reason", "accepted_by"}, ("path", "reason", "accepted_by"))
        need(e.get("index") in ("pypi", "crates.io", "npm"), w + ": index must be pypi, crates.io or npm")
        need(e.get("class") == "PUBLIC-UPSTREAM-ARCHIVE", w + ": class must be PUBLIC-UPSTREAM-ARCHIVE")
    for n, e in enumerate(files):
        w = "acceptance files[%d]" % n
        entry(e, w, {"path", "sha256", "class", "reason", "accepted_by"}, ("path", "reason", "accepted_by"))
        need(e.get("class") == "PUBLIC-UPSTREAM-FILE", w + ": class must be PUBLIC-UPSTREAM-FILE")
    for n, e in enumerate(unop):
        entry(e, "acceptance unopened[%d]" % n, {"path", "sha256", "reason", "accepted_by"}, ("path", "reason", "accepted_by"))
    return pos, arch, files, unop


def validate_pending(doc):
    need(isinstance(doc, dict) and doc.get("schema") == SCHEMA_PENDING, "pending list: schema must be %s" % SCHEMA_PENDING)
    need(set(doc) <= {"schema", "positions", "note"}, "pending list: unknown top-level key")
    pos = doc.get("positions", [])
    need(isinstance(pos, list), "pending list: positions must be a list")
    for n, e in enumerate(pos):
        w = "pending positions[%d]" % n
        entry(e, w, {"rule", "path", "line", "sha256", "note", "count"}, ("rule", "path"))
        need(isinstance(e.get("line"), int) and not isinstance(e.get("line"), bool), w + ": line must be an integer")
        need(positive(e.get("count", 1)), w + ": count must be a positive integer")
    return pos


def load_extractor():
    with open(EXTRACTOR, "rb") as f:
        raw = f.read()
    module = types.ModuleType("release_payload_deep_extract")
    module.__file__ = EXTRACTOR
    exec(compile(raw, EXTRACTOR, "exec"), module.__dict__)
    return module


def chain_of(rel, outdirs):
    """Tree-relative path -> the path with "!" at archive boundaries, and the enclosing archives
    as (tree-relative path of the archive file, its path with "!")."""
    comps = [c for c in rel.split("/") if c]
    chain, cur, prefix, ancestors = [], [], [], []
    for comp in comps:
        prefix.append(comp)
        cur.append(comp)
        key = "/".join(prefix)
        if key in outdirs:
            cur[-1] = os.path.basename(outdirs[key])
            chain.append("/".join(cur))
            ancestors.append((outdirs[key], "!".join(chain)))
            cur = []
    if cur:
        chain.append("/".join(cur))
    return "!".join(chain), ancestors


def strip_allowlist(default_text):
    """Remove global path-allowlist lines that mention vendor, dist-info or lock (lock files).
    Returns (text, removed)."""
    out, removed = [], 0
    in_global = in_paths = False
    for ln in default_text.splitlines(keepends=True):
        s = ln.strip()
        if s.startswith("["):
            in_global = s in ("[allowlist]", "[[allowlists]]")
            in_paths = False
        elif in_global and re.match(r"^paths\s*=\s*\[", s):
            in_paths = not s.endswith("]")
        elif in_global and in_paths:
            if s.startswith("]"):
                in_paths = False
            elif "vendor" in s or "dist-info" in s or "lock" in s:
                removed += 1
                continue
        out.append(ln)
    return "".join(out), removed


def global_paths(cfg_text):
    try:
        cfg = tomllib.loads(cfg_text)
    except tomllib.TOMLDecodeError:
        raise Failure("a default rules file is not TOML")
    pats = list((cfg.get("allowlist") or {}).get("paths", []))
    for extra in cfg.get("allowlists", []) or []:
        if not extra.get("targetRules"):
            pats.extend(extra.get("paths", []))
    out = []
    for p in pats:
        try:
            out.append(re.compile(p))
        except re.error:
            raise Failure("a detector path-allowlist pattern cannot be evaluated by this tool")
    return out


class Build:
    def __init__(self, role, exe, label, default_cfg):
        self.role, self.exe, self.label, self.default_cfg = role, exe, label, default_cfg
        self.sha256 = sha256_file(exe)
        self.dir_mode = False
        self.cfg = self.default_copy = None
        self.removed = 0
        self.patterns = []


def run_detector(build, source, config, report, logdir, tag, empty_cwd):
    """One run of a build over the directory source. Returns its findings (redacted by the build)."""
    if build.dir_mode:
        cmd = [build.exe, "dir", source]
    else:
        cmd = [build.exe, "detect", "--no-git", "--source", source]
    if config:
        cmd += ["--config", config]
    cmd += ["--redact=100", "--no-banner", "--no-color", "--log-level", "error", "--ignore-gitleaks-allow",
            "--report-format", "json", "--report-path", report, "--exit-code", str(LEAK_EXIT)]
    env = {k: v for k, v in os.environ.items() if k not in ("GITLEAKS_CONFIG", "GITLEAKS_CONFIG_TOML")}
    so, se = os.path.join(logdir, tag + ".stdout"), os.path.join(logdir, tag + ".stderr")
    fo = os.open(so, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    fe = os.open(se, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    try:
        rc = subprocess.run(cmd, stdout=fo, stderr=fe, stdin=subprocess.DEVNULL, cwd=empty_cwd, env=env, check=False).returncode
    finally:
        os.close(fo)
        os.close(fe)
    if os.path.exists(report):
        os.chmod(report, 0o600)
    if rc not in (0, LEAK_EXIT) or not os.path.exists(report) or os.path.getsize(se) != 0:
        raise Failure("the %s build's run %s did not finish cleanly (exit %d)" % (build.role, tag, rc))
    with open(report, "rb") as f:
        data = json.load(f) or []
    if not isinstance(data, list) or not all(isinstance(item, dict) for item in data):
        raise Failure("the %s build's run %s wrote no list of findings" % (build.role, tag))
    return data


def number(item, key):
    value = item.get(key, 0)
    if isinstance(value, bool) or not isinstance(value, int):
        raise Failure("a finding without a line number")
    return value


def prepare_builds(args):
    builds = []
    for role, given, cfg, label in (("pinned", args.gitleaks_pinned, args.default_config_pinned, args.pinned_version),
                                    ("current", args.gitleaks_current, args.default_config_current, args.current_version)):
        need(os.path.isabs(given), "the %s build must be given by absolute path" % role)
        exe = os.path.realpath(given)
        need(os.path.isfile(exe) and os.access(exe, os.X_OK), "the %s build is not an executable file" % role)
        if not cfg:
            cfg = os.path.join(os.path.dirname(given), "gitleaks.toml")
            need(os.path.isfile(cfg), "no gitleaks.toml beside the %s build: pass --default-config-%s" % (role, role))
        need(os.path.isfile(cfg), "the default rules file of the %s build was not found" % role)
        if not label:
            out = subprocess.run([exe, "version"], stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, stdin=subprocess.DEVNULL,
                                 cwd="/", check=False).stdout.decode("utf-8", "replace").strip()
            label = out if re.fullmatch(r"v?[0-9]+\.[0-9]+\.[0-9]+", out) else "unknown-" + sha256_file(exe)[:12]
        need(re.fullmatch(r"[A-Za-z0-9._-]{1,40}", label), "a version label is not printable")
        builds.append(Build(role, exe, label.lstrip("v"), cfg))
    need(builds[0].sha256 != builds[1].sha256, "the pinned and the current build are the same file")
    return builds


def write_new(path, text):
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(fd, "w", encoding="utf-8") as f:
        f.write(text)


class Scan:
    """The state of one run: what the builds found and what nothing read."""

    def __init__(self, work, builds, extractor):
        self.work, self.builds, self.extractor = work, builds, extractor
        self.empty_cwd = os.path.join(work, "empty-cwd")
        self.positions = collections.OrderedDict()
        self.payloads = []
        self.unopened = []     # (payload, path, format, sha256, ancestors, tree)
        self.unread = []       # (payload, path, reason, sha256, ancestors, tree)
        self.skipped = []      # (payload, archive path, sha256, reason, count, ancestors, tree)
        self.artifacts = []
        self.unread_reasons = collections.Counter()
        self.elf_files = 0
        self.sha_cache = {}

    def fsha(self, path):
        if path not in self.sha_cache:
            self.sha_cache[path] = sha256_file(path)
        return self.sha_cache[path]

    def payload(self, i, payload):
        builds, D = self.builds, self.extractor
        pdir = os.path.join(self.work, "p%d" % i)
        scan_root = os.path.join(pdir, "scan")
        os.makedirs(scan_root, mode=0o700)
        tree = os.path.join(scan_root, "t")
        for b in builds:
            if any(rx.search(tree + "/x") for rx in b.patterns):
                raise Usage("the work directory matches a detector path-allowlist pattern; choose another --work")
        try:
            stats = D.expand_file(payload, tree)
        except D.ExtractError:
            raise Failure("deep extraction stopped (disk guard, size cap or output location)")
        base = os.path.basename(payload)
        digest = stats["sha256"]    # of the copy that was opened and scanned, not of a later read of the file
        self.sha_cache[os.path.join(tree, base)] = digest
        outdirs = {}
        for rec in stats["archives"]:
            d = os.path.dirname(rec["path"])
            outdirs[os.path.join(d, rec["outdir"]) if d else rec["outdir"]] = rec["path"]
        top = next((rec for rec in stats["archives"] if rec["path"] == base), None)
        top_dir = None
        if top is not None:
            # The payload is an opened archive: its members stand for it, at paths below its top directory.
            os.unlink(os.path.join(tree, base))
            inner = os.path.join(tree, top["outdir"])
            names = os.listdir(inner)
            if len(names) == 1 and os.path.isdir(os.path.join(inner, names[0])) and not os.path.islink(os.path.join(inner, names[0])):
                top_dir = names[0]

        def strip(cpath):
            segs = cpath.split("!")
            if top is not None and len(segs) > 1 and segs[0] == base:
                segs = segs[1:]
                if top_dir and segs[0].startswith(top_dir + "/"):
                    segs[0] = segs[0][len(top_dir) + 1:]
            return "!".join(segs)

        def canon(rel):
            """The path inside the payload, and the enclosing archives (the payload file last out)."""
            cpath, ancestors = chain_of(rel, outdirs)
            return strip(cpath), [(arel, acanon if arel == base else strip(acanon)) for arel, acanon in ancestors]

        closed = {rec["path"] for rec in stats["not_opened"] + stats["partial"]}
        for rec in stats["not_opened"] + stats["partial"]:
            cpath, ancestors = canon(rec["path"])
            self.unopened.append((i, cpath, rec.get("format", "?"), self.fsha(os.path.join(tree, rec["path"])), ancestors, tree))
        skips = collections.Counter()
        for rec in stats["archives"]:
            for key, count in rec.get("skips", {}).items():
                skips[key] += count
                reason = key.split(":", 1)[-1]
                if reason not in NO_CONTENT_SKIPS and rec["path"] not in closed:
                    cpath, ancestors = canon(rec["path"])
                    self.skipped.append((i, cpath, self.fsha(os.path.join(tree, rec["path"])), reason, count, ancestors, tree))
        opened = {rec["path"] for rec in stats["archives"]} | closed

        # What no build reads, and the printable runs of the compiled files.
        sroot = os.path.join(pdir, "strings")
        os.makedirs(os.path.join(sroot, "t"), mode=0o700)
        elf_by_sha, elf_by_idx = {}, {}
        files = 0
        for dirpath, dirnames, filenames in os.walk(tree, followlinks=False):
            dirnames.sort()
            for fn in sorted(filenames):
                p = os.path.join(dirpath, fn)
                if os.path.islink(p) or not os.path.isfile(p):
                    continue
                files += 1
                rel = os.path.relpath(p, tree)
                if rel in opened or os.path.getsize(p) == 0:
                    continue
                with open(p, "rb") as f:
                    head = f.read(600)
                if head[:4] == b"\x7fELF":
                    esha = self.fsha(p)
                    if esha not in elf_by_sha:
                        with open(p, "rb") as f:
                            body = f.read()
                        offs, idx = [], len(elf_by_sha)
                        fd = os.open(os.path.join(sroot, "t", "%05d.txt" % idx), os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
                        with os.fdopen(fd, "wb") as f:
                            for run in STRINGS_RX.finditer(body):
                                offs.append(run.start())
                                f.write(run.group(0) + b"\n")
                        del body
                        elf_by_sha[esha] = {"idx": idx, "offs": offs, "paths": []}
                        elf_by_idx[idx] = esha
                    elf_by_sha[esha]["paths"].append(rel)
                    self.elf_files += 1
                    continue
                if any(head[off:off + len(m)] == m for off, m in BIN_MAGICS):
                    reason = "binary signature (not an opened archive)"
                else:
                    hit = [any(rx.search(p) for rx in b.patterns) for b in builds]
                    if not any(hit):
                        continue
                    # The rule is per build: a file one build skips is unread by that build.
                    reason = "on both builds' remaining path allowlists" if all(hit) else \
                        "on one build's remaining path allowlist (the other build reads it)"
                self.unread_reasons[reason] += 1
                cpath, ancestors = canon(rel)
                self.unread.append((i, cpath, reason, self.fsha(p), ancestors, tree))

        rdir = os.path.join(pdir, "reports")
        os.makedirs(rdir, mode=0o700)
        raw = collections.OrderedDict()
        for b in builds:
            data = run_detector(b, scan_root, b.cfg, os.path.join(rdir, "%s.report.json" % b.role), rdir, b.role, self.empty_cwd)
            raw[b.label] = len(data)
            if i == 0:
                # The default rules file given for the build must be the build's own default.
                def keyset(items):
                    return collections.Counter((str(x.get("RuleID")), str(x.get("File")), x.get("StartLine")) for x in items)
                d1 = run_detector(b, scan_root, None, os.path.join(rdir, "%s-embedded-default.report.json" % b.role), rdir,
                                  b.role + "-embedded-default", self.empty_cwd)
                d2 = run_detector(b, scan_root, b.default_copy, os.path.join(rdir, "%s-explicit-default.report.json" % b.role), rdir,
                                  b.role + "-explicit-default", self.empty_cwd)
                if keyset(d1) != keyset(d2):
                    raise Failure("the default rules file given for the %s build does not reproduce its embedded default" % b.role)
                del d1, d2
            for it in data:
                fpath = str(it.get("File", ""))
                if not fpath.startswith(tree + "/"):
                    raise Failure("a finding lies outside the scan tree")
                rel = fpath[len(tree) + 1:]
                cpath, ancestors = canon(rel)
                key = (i, str(it.get("RuleID", "?")), cpath, number(it, "StartLine"))
                rec = self.positions.setdefault(key, {"member": fpath, "ancestors": ancestors, "tree": tree,
                                                      "per_build": collections.Counter(), "strings": False})
                rec["per_build"][b.label] += 1
            del data
            # The printable runs of the ELF files. Pass 1 reads the dumps, one run per line (fast). A rule can
            # join two neighbouring runs across the line break, so pass 2 reads EVERY run a pass-1 finding
            # touches as a file of its own, and only pass-2 findings count. A pass-1 finding that no single
            # run reproduces is recorded as a method artifact.
            sdata = run_detector(b, sroot, b.cfg, os.path.join(rdir, "%s-strings.report.json" % b.role), rdir, b.role + "-strings", self.empty_cwd)
            raw[b.label + " ELF strings (runs joined by line)"] = len(sdata)
            touched = collections.defaultdict(set)
            joined = []
            for it in sdata:
                mfile = re.search(r"/(\d{5})\.txt$", str(it.get("File", "")))
                if not mfile or int(mfile.group(1)) not in elf_by_idx:
                    raise Failure("a strings finding lies outside the dump directory")
                idx = int(mfile.group(1))
                nruns = len(elf_by_sha[elf_by_idx[idx]]["offs"])
                sl = number(it, "StartLine")
                el = max(sl, number(it, "EndLine"))
                if sl < 1 or sl > nruns:
                    raise Failure("a strings finding has no run")
                lo, hi = max(1, sl - 2), min(nruns, el + 2)
                if hi - lo > 20000:
                    raise Failure("a strings finding spans too many runs to isolate")
                touched[idx].update(range(lo, hi + 1))
                joined.append((idx, str(it.get("RuleID", "?")), sl, lo, hi))
            del sdata
            iso_root = os.path.join(pdir, "strings-isolated-" + b.role)
            os.makedirs(os.path.join(iso_root, "t"), mode=0o700)
            for idx, lines in touched.items():
                with open(os.path.join(sroot, "t", "%05d.txt" % idx), "rb") as f:
                    all_lines = f.read().split(b"\n")
                for ln in sorted(lines):
                    fd = os.open(os.path.join(iso_root, "t", "%05d-%08d.txt" % (idx, ln)), os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
                    with os.fdopen(fd, "wb") as f:
                        f.write(all_lines[ln - 1] + b"\n")
                del all_lines
            idata = run_detector(b, iso_root, b.cfg, os.path.join(rdir, "%s-strings-isolated.report.json" % b.role), rdir,
                                 b.role + "-strings-isolated", self.empty_cwd) if touched else []
            raw[b.label + " ELF strings (each run alone)"] = len(idata)
            iso_hits = set()
            for it in idata:
                mfile = re.search(r"/(\d{5})-(\d{8})\.txt$", str(it.get("File", "")))
                if not mfile or int(mfile.group(1)) not in elf_by_idx:
                    raise Failure("an isolated strings finding lies outside the dump directory")
                idx, ln = int(mfile.group(1)), int(mfile.group(2))
                rule = str(it.get("RuleID", "?"))
                ent = elf_by_sha[elf_by_idx[idx]]
                if ln < 1 or ln > len(ent["offs"]):
                    raise Failure("an isolated strings finding has no run")
                iso_hits.add((idx, rule, ln))
                for erel in ent["paths"]:
                    cpath, ancestors = canon(erel)
                    key = (i, rule, cpath + STRINGS, ent["offs"][ln - 1])
                    rec = self.positions.setdefault(key, {"member": os.path.join(tree, erel), "ancestors": ancestors, "tree": tree,
                                                          "per_build": collections.Counter(), "strings": True})
                    rec["per_build"][b.label] += 1
            del idata
            for idx, rule, sl, lo, hi in joined:
                if not any((idx, rule, ln) in iso_hits for ln in range(lo, hi + 1)):
                    ent = elf_by_sha[elf_by_idx[idx]]
                    for erel in ent["paths"]:
                        self.artifacts.append((i, b.label, canon(erel)[0], rule, ent["offs"][sl - 1]))
        self.payloads.append({"name": base, "sha256": digest, "bytes": stats["bytes"], "files": files,
                              "archives_opened": sum(stats["opened_per_format"].values()),
                              "archives_not_opened": len(closed), "member_skips": dict(sorted(skips.items())),
                              "raw_findings": dict(raw)})


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--name", required=True, help="the name of this run in the receipt")
    ap.add_argument("--gitleaks-pinned", required=True, help="absolute path of the pinned older build")
    ap.add_argument("--gitleaks-current", required=True, help="absolute path of the current build")
    ap.add_argument("--default-config-pinned", help="the pinned build's default rules (default: gitleaks.toml beside it)")
    ap.add_argument("--default-config-current", help="the current build's default rules (default: gitleaks.toml beside it)")
    ap.add_argument("--pinned-version", help="a label for a pinned build that does not print its version")
    ap.add_argument("--current-version", help="a label for a current build that does not print its version")
    ap.add_argument("--accepted-findings", help="the private acceptance file (optional; without it nothing is accepted)")
    ap.add_argument("--pending", help="the pending list (optional; it never accepts)")
    ap.add_argument("--receipt", required=True, help="the counts-only receipt (a new file)")
    ap.add_argument("--report", required=True, help="the counts-only report, JSON (a new file)")
    ap.add_argument("--unaccepted", help="what refused, by path, position and sha256, without content (a new file, 0600)")
    ap.add_argument("--work", help="parent of the private work directory")
    ap.add_argument("--keep-work", action="store_true")
    ap.add_argument("--proof-only", action="store_true",
                    help="a run that is no release receipt: its name gets the PROOF-ONLY- prefix and its report says release false")
    ap.add_argument("--min-free-bytes", type=int, default=1 << 30, help="stop when the work file system has less free space")
    ap.add_argument("--disk-stop-percent", type=float, help="stop when the work file system is at least this full")
    ap.add_argument("payload", nargs="*")
    a = ap.parse_args()
    os.umask(0o077)
    need(re.fullmatch(r"[A-Za-z0-9._-]{1,80}", a.name), "--name must match [A-Za-z0-9._-]{1,80}")
    need(not a.name.startswith(PROOF_ONLY), "a name that starts with PROOF-ONLY is given by --proof-only alone")
    if a.proof_only:
        a.name = PROOF_ONLY + "-" + a.name
    need(a.payload, "at least one payload file is required")
    payloads = [os.path.abspath(p) for p in a.payload]
    for p in payloads:
        need(os.path.isfile(p) and not os.path.islink(p), "a payload is not a regular file")
        need(re.fullmatch(r"[A-Za-z0-9._+-]+", os.path.basename(p)), "a payload file name has characters a receipt cannot hold")
    need(len({os.path.basename(p) for p in payloads}) == len(payloads), "two payload files have the same name")
    outputs = [os.path.abspath(p) for p in (a.receipt, a.report, a.unaccepted) if p]
    need(not any(os.path.lexists(p) for p in outputs) and len(set(outputs)) == len(outputs),
         "the receipt, the report and the listing must be new files")
    acc_pos = acc_arch = acc_files = acc_unop = pend = ()
    acc_sha = pend_sha = "none"
    if a.accepted_findings:
        doc, acc_sha = load_json(a.accepted_findings, "acceptance file")
        acc_pos, acc_arch, acc_files, acc_unop = validate_accepted(doc)
    if a.pending:
        doc, pend_sha = load_json(a.pending, "pending list")
        pend = validate_pending(doc)
    need(os.path.isfile(EXTRACTOR), "release-payload-deep-extract.py is missing beside this file")
    need(a.work is None or os.path.isdir(a.work), "--work must be an existing directory")
    builds = prepare_builds(a)
    started = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())
    commit = tool_commit()

    work = tempfile.mkdtemp(prefix="credential-scan-", dir=a.work)
    try:
        os.chmod(work, 0o700)
        cfg_dir = os.path.join(work, "config")
        os.makedirs(cfg_dir, mode=0o700)
        extractor = load_extractor()
        extractor.MIN_FREE_BYTES, extractor.DISK_STOP_PERCENT = a.min_free_bytes, a.disk_stop_percent
        scan = Scan(work, builds, extractor)
        os.makedirs(scan.empty_cwd, mode=0o700)
        for b in builds:
            with open(b.default_cfg, "rb") as f:
                raw = f.read()
            text = raw.decode("utf-8")
            b.default_sha256 = hashlib.sha256(raw).hexdigest()
            stripped, b.removed = strip_allowlist(text)
            b.default_copy = os.path.join(cfg_dir, "default-%s.toml" % b.role)
            b.cfg = os.path.join(cfg_dir, "default-minus-removed-paths-%s.toml" % b.role)
            write_new(b.default_copy, text)
            write_new(b.cfg, stripped)
            b.patterns = global_paths(stripped)
            b.dir_mode = subprocess.run([b.exe, "dir", "--help"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                                        stdin=subprocess.DEVNULL, cwd=scan.empty_cwd, check=False).returncode == 0
        for i, payload in enumerate(payloads):
            scan.payload(i, payload)
        return decide(a, scan, (acc_pos, acc_arch, acc_files, acc_unop), acc_sha, pend, pend_sha, started, commit)
    finally:
        if not a.keep_work:
            shutil.rmtree(work, ignore_errors=True)


def decide(a, scan, accepted, acc_sha, pend, pend_sha, started, commit):
    acc_pos, acc_arch, acc_files, acc_unop = accepted
    builds = scan.builds

    def in_archive(ancestors, tree):
        return any(e["path"] == acanon and e["sha256"] == scan.fsha(os.path.join(tree, arel))
                   for arel, acanon in ancestors for e in acc_arch)

    def in_file(path, sha):
        return any(e["path"] == path and e["sha256"] == sha for e in acc_files)

    counts = collections.Counter()
    per_rule = collections.defaultdict(collections.Counter)
    refused = []    # (payload, rule, path, line, findings, pending owner, sha256)
    for (i, rule, cpath, line), rec in scan.positions.items():
        msha = scan.fsha(rec["member"])
        count = max(rec["per_build"].values())

        def same(e):
            return e["rule"] == rule and e["path"] == cpath and e["line"] == line and e["sha256"] == msha and e.get("count", 1) == count

        is_pend = any(same(e) for e in pend)
        if is_pend:
            decision = "refused"    # a pending position refuses whatever the acceptance file says
        elif any(same(e) for e in acc_pos):
            decision = "accepted position"
        elif in_archive(rec["ancestors"], rec["tree"]):
            decision = "accepted in archive"
        elif in_file(cpath[:-len(STRINGS)] if rec["strings"] else cpath, msha):
            decision = "accepted in file"
        else:
            decision = "refused"
        if decision == "refused":
            refused.append((i, rule, cpath, line, count, is_pend, msha))
        counts["findings"] += count
        counts[decision] += count
        per_rule[rule][decision] += count
    unopened_refused = [row for row in scan.unopened
                        if not (any(e["path"] == row[1] and e["sha256"] == row[3] for e in acc_unop)
                                or in_file(row[1], row[3]) or in_archive(row[4], row[5]))]
    for i, cpath, _fmt, usha, _anc, _tree in unopened_refused:
        refused.append((i, UNOPENED_RULE, cpath, 0, 1, False, usha))
    unread_refused = [row for row in scan.unread if not (in_file(row[1], row[3]) or in_archive(row[4], row[5]))]
    skipped_refused = [row for row in scan.skipped
                       if not (any(e["path"] == row[1] and e["sha256"] == row[2] for e in acc_arch) or in_archive(row[5], row[6]))]
    n_refused = sum(r[4] for r in refused)
    n_pending = sum(r[4] for r in refused if r[5])
    n_unread = len(scan.unread) + len(scan.unopened) + sum(row[4] for row in scan.skipped)
    n_unread_unaccepted = len(unread_refused) + sum(row[4] for row in skipped_refused)
    verdict = "PASS" if n_refused == 0 and n_pending == 0 and n_unread_unaccepted == 0 else "REFUSED"

    line1 = "CREDENTIAL-LEG %s %s files=%d %s accepted=%s pending=%s builds=%s build_sha256=%s findings=%d accepted_positions=%d " \
            "accepted_in_archives=%d accepted_in_files=%d refused=%d pending_owner=%d unread=%d unread_unaccepted=%d" % (
                verdict, a.name, len(scan.payloads), " ".join("%s=%s" % (p["name"], p["sha256"]) for p in scan.payloads),
                acc_sha, pend_sha, ",".join(b.label for b in builds), ",".join(b.sha256 for b in builds), counts["findings"],
                counts["accepted position"], counts["accepted in archive"], counts["accepted in file"], n_refused, n_pending,
                n_unread, n_unread_unaccepted)
    out = [line1, ""]
    out.append("counts are findings: a position (rule, path, line) with N findings counts N (the larger of the two builds); "
               "distinct positions: %d; an archive that could not be opened counts as one refusal" % len(scan.positions))
    out.append("")
    out.append("| payload | files in deep tree | archives opened | archives not opened | raw findings per build | member skips |")
    out.append("|---|---|---|---|---|---|")
    for i, p in enumerate(scan.payloads):
        out.append("| p%d %s | %d | %d | %d | %s | %s |" % (
            i, p["name"], p["files"], p["archives_opened"], p["archives_not_opened"],
            ", ".join("%s=%d" % kv for kv in p["raw_findings"].items()),
            "; ".join("%s: %d" % kv for kv in p["member_skips"].items()) or "none"))
    out.append("")
    out.append("| build | role | binary sha256 | allowlist lines removed (vendor, dist-info, lock) | default rules proved against the embedded default |")
    out.append("|---|---|---|---|---|")
    for b in builds:
        out.append("| %s | %s | %s | %d | yes |" % (b.label, b.role, b.sha256, b.removed))
    out.append("")
    out.append("| rule | findings | accepted position | accepted in archive | accepted in file | refused |")
    out.append("|---|---|---|---|---|---|")
    for rule in sorted(per_rule):
        c = per_rule[rule]
        out.append("| %s | %d | %d | %d | %d | %d |" % (rule, sum(c.values()), c["accepted position"], c["accepted in archive"],
                                                    c["accepted in file"], c["refused"]))
    out.append("")
    out.append("ELF files covered by their printable runs (8 or more printable ASCII bytes, each run a unit of its own): %d" % scan.elf_files)
    out.append("unread files: %s; of them without an acceptance entry: %d" % (
        "; ".join("%s: %d" % kv for kv in sorted(scan.unread_reasons.items())) or "none", len(unread_refused)))
    out.append("archives not opened: %d (without an acceptance entry: %d); members not written out that hold content: %d "
               "(without an acceptance entry: %d)" % (
                   len(scan.unopened), len(unopened_refused), sum(row[4] for row in scan.skipped),
                   sum(row[4] for row in skipped_refused)))
    out.append("acceptance file: %d position entries, %d archive entries, %d file entries, %d unopened entries; "
               "pending entries (never accepting): %d" % (len(acc_pos), len(acc_arch), len(acc_files), len(acc_unop), len(pend)))
    out.append("")
    out.append("method artifact: %d findings that span two runs (reported only when the runs are joined by line; "
               "no single run reproduces them; not counted, not refused)" % len(scan.artifacts))
    if scan.artifacts:
        out.append("")
        out.append("| payload | build | ELF path | rule | offset of the run |")
        out.append("|---|---|---|---|---|")
        for i, blabel, cpath, rule, offset in sorted(scan.artifacts):
            out.append("| p%d | %s | %s | %s | %d |" % (i, blabel, cpath.replace("|", "/"), rule, offset))
    out.append("")
    out.append("| refused: payload | rule | path | line | findings | pending owner |")
    out.append("|---|---|---|---|---|---|")
    for i, rule, cpath, line, count, is_pend, _sha in sorted(refused):
        out.append("| p%d | %s | %s | %d | %d | %s |" % (i, rule, cpath.replace("|", "/"), line, count, "yes" if is_pend else "no"))
    if not refused:
        out.append("| none | | | | | |")
    write_new(os.path.abspath(a.receipt), "\n".join(out) + "\n")

    names = [p["name"] for p in scan.payloads]
    report = {
        "schema": SCHEMA_REPORT, "name": a.name, "label": a.name, "release": not a.proof_only, "status": verdict,
        "generated_at": started, "tool_commit": commit, "tool_sha256": sha256_file(os.path.abspath(__file__)),
        "receipt": line1,
        "payload": scan.payloads,
        "detector_builds": [{"role": b.role, "version": b.label, "sha256": b.sha256, "default_rules_sha256": b.default_sha256,
                             "allowlist_lines_removed": b.removed} for b in builds],
        "accepted": {"count": len(acc_pos) + len(acc_arch) + len(acc_files) + len(acc_unop),
                     "sha256": None if acc_sha == "none" else acc_sha,
                     "positions": len(acc_pos), "archives": len(acc_arch), "files": len(acc_files), "unopened": len(acc_unop)},
        "pending": {"count": len(pend), "sha256": None if pend_sha == "none" else pend_sha},
        "findings": {"total": counts["findings"], "positions": len(scan.positions), "accepted_positions": counts["accepted position"],
                     "accepted_in_archives": counts["accepted in archive"], "accepted_in_files": counts["accepted in file"],
                     "refused": n_refused, "pending_owner": n_pending,
                     "by_rule": {rule: dict(sorted(c.items())) for rule, c in sorted(per_rule.items())}},
        "unread": {"total": n_unread, "unaccepted": n_unread_unaccepted, "files": dict(sorted(scan.unread_reasons.items())),
                   "archives_not_opened": len(scan.unopened), "archives_not_opened_unaccepted": len(unopened_refused),
                   "members_not_written": sum(row[4] for row in scan.skipped)},
        "elf_files": scan.elf_files, "method_artifacts": len(scan.artifacts),
    }
    write_new(os.path.abspath(a.report), json.dumps(report, indent=2, sort_keys=True) + "\n")
    if a.unaccepted:
        listing = {
            "schema": SCHEMA_UNACCEPTED,
            "positions": [{"payload": names[i], "rule": rule, "path": cpath, "sha256": sha, "line": line, "count": count, "pending": is_pend}
                          for i, rule, cpath, line, count, is_pend, sha in sorted(refused) if rule != UNOPENED_RULE],
            "unopened": [{"payload": names[i], "path": cpath, "sha256": usha, "format": fmt}
                         for i, cpath, fmt, usha, _anc, _tree in sorted(unopened_refused, key=lambda row: row[:4])],
            "unread": [{"payload": names[i], "path": cpath, "sha256": sha, "reason": reason}
                       for i, cpath, reason, sha, _anc, _tree in sorted(unread_refused, key=lambda row: row[:4])],
            "members_not_written": [{"payload": names[i], "archive": cpath, "sha256": sha, "reason": reason, "count": count}
                                    for i, cpath, sha, reason, count, _anc, _tree in sorted(skipped_refused, key=lambda row: row[:5])],
        }
        write_new(os.path.abspath(a.unaccepted), json.dumps(listing, indent=1, sort_keys=True) + "\n")
    print(line1)
    return 0 if verdict == "PASS" else 1


if __name__ == "__main__":
    try:
        sys.exit(main())
    except Usage as exc:
        sys.stderr.write("release-payload-credential-scan: usage error: %s\n" % exc)
        sys.exit(2)
    except Failure as exc:
        sys.stderr.write("CREDENTIAL-LEG REFUSED tool-failure: %s\n" % exc)
        sys.exit(3)
    except SystemExit:
        raise
    except BaseException as exc:  # noqa: BLE001 - never echo exception text (it may hold data)
        tb = traceback.extract_tb(exc.__traceback__)
        sys.stderr.write("CREDENTIAL-LEG REFUSED tool-failure: %s at %s\n" % (
            type(exc).__name__, ", ".join("%s:%d" % (os.path.basename(fr.filename), fr.lineno) for fr in tb[-3:])))
        sys.exit(3)
