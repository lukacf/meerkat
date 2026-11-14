"""Code identifier entry: the classes of name-list hits in a JavaScript member and the one entry the class table allows.

Synthetic fixtures only: every class, token, key, archive and upstream tarball below is made up
and built in a temporary directory. No real list, rule, package or release byte is used.
"""
from pathlib import Path
import base64
import contextlib
import gzip
import hashlib
import importlib.util
import io
import json
import os
import re
import shutil
import subprocess
import sys
import tarfile
import tempfile
import unittest
import zipfile

HERE = Path(__file__).resolve().parent
TOOL = HERE / "release-code-identifier-entry.py"
NAMES = "# made-up names\nzqclass\tzqfixturename\nreview-zqword\tzqreviewed\n"
TOKEN = "zqfixturename"
MEMBER = "pkg/libexec/library.tgz!package/dist/app.js"
WHO = "zq reviewer"
UPSTREAM_WHO = "zq upstream reviewer"
SIZE_WORDS = {"len", "length", "size", "width", "span", "end", "byte", "bytes"}
IDENTIFIERS = (b"function zqfixturename(a){return a}\nvar b=zqfixturename(1),c=ZqFixtureName;\n"
               b"var t=`x ${zqfixturename} y`;\n")
DATABASE = b'var db={"zqkey":{zqlist:["zqplain",\'zqfixturename\'],flag:!0},"zqnext":{zqlist:[],flag:!1}};\n'
UPSTREAM = {"zqkey": {"zqlist": ["zqplain", "zqfixturename"], "flag": True}, "zqnext": {"zqlist": [], "flag": False}}
# One fixture per row of the class table: member bytes whose one hit reaches the class.
CLASS_FIXTURES = {
    "COINCIDENTAL-CODE-IDENTIFIER": [b"var a=zqfixturename;\n"],
    "PUBLIC-UPSTREAM-CONTENT": [DATABASE],
    "STRING-LITERAL-NOT-PROVED": [b'var s="zqfixturename";\n', b"var s=['zqfixturename'];\n",
                                  DATABASE.replace(b"'zqfixturename'", b"'zqfixturename x'")],
    "TEMPLATE-LITERAL-TEXT": [b"var t=`a zqfixturename b`;\n", b"var t=`${a} zqfixturename`;\n"],
    "COMMENT": [b"var a=1; // zqfixturename\n", b"/* zqfixturename */var a=1;\n"],
    "REGEX-LITERAL": [b"var r=/zqfixturename/g;\n", b"return /[/]zqfixturename/.test(a)\n"],
    "PROPERTY-NAME": [b"var o={};o.zqfixturename=1;\n", b"var o={zqfixturename:1};\n", b"var o={a:1, zqfixturename :2};\n",
                      b"o?.zqfixturename\n", b"zqfixturename:for(;;){}\n"],
    "LONGER-IDENTIFIER": [b"var _zqfixturename=1;\n", b"var zqfixturename$=1;\n", b"zqfixturename2()\n"],
}


def tar_bytes(members):
    raw = io.BytesIO()
    with tarfile.open(fileobj=raw, mode="w", format=tarfile.PAX_FORMAT) as archive:
        for name, data in members.items():
            info = tarfile.TarInfo(name)
            info.size = len(data)
            archive.addfile(info, io.BytesIO(data))
    return gzip.compress(raw.getvalue(), mtime=0)


def zip_bytes(members):
    raw = io.BytesIO()
    with zipfile.ZipFile(raw, "w", zipfile.ZIP_DEFLATED) as archive:
        for name, data in members.items():
            archive.writestr(zipfile.ZipInfo(name, (1980, 1, 1, 0, 0, 0)), data)
    return raw.getvalue()


def product_bytes(member):
    """A product archive that embeds a library archive that holds the JavaScript member."""
    return tar_bytes({"pkg/bin/tool": b"inert tool\n", "pkg/libexec/library.tgz": tar_bytes(
        {"package/package.json": b"{}\n", "package/dist/app.js": member})})


def integrity(data):
    return "sha512-" + base64.b64encode(hashlib.sha512(data).digest()).decode()


def load(name, path):
    flag = sys.dont_write_bytecode
    try:
        spec = importlib.util.spec_from_file_location(name, path)
        value = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(value)
    finally:
        sys.dont_write_bytecode = flag
    return value


def run(script, *args):
    return subprocess.run([sys.executable, "-I", str(script), *map(str, args)], capture_output=True, text=True)


def call(module, *args):
    """The tool's run() in this process (for a patched class table): (exit code, stdout, stderr)."""
    out, err = io.StringIO(), io.StringIO()
    mask = os.umask(0o077)
    try:
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            try:
                code = module.run([str(arg) for arg in args])
            except SystemExit as stop:
                code = stop.code
    finally:
        os.umask(mask)
    return code, out.getvalue(), err.getvalue()


def census(stdout):
    return {name: int(count) for name, count in re.findall(r"^  ([A-Z-]+): (\d+) - ", stdout, re.M)}


def keys_of(value, names=False):
    """Every field key of every mapping inside a JSON value. The keys of the maps `census` and
    `classes` are class names, not fields: they are skipped."""
    if isinstance(value, dict):
        for key, inner in value.items():
            if not names:
                yield key
            yield from keys_of(inner, not names and key in ("census", "classes"))
    elif isinstance(value, list):
        for inner in value:
            yield from keys_of(inner)


class Fixture(unittest.TestCase):
    """A made-up name list, a payload archive around one JavaScript member and an upstream tarball."""

    def setUp(self):
        self.root = Path(tempfile.mkdtemp(prefix="identifier-entry-test-")).resolve()
        self.addCleanup(shutil.rmtree, self.root, True)
        self.count = 0
        self.names = self.root / "names.txt"
        self.names.write_text(NAMES, encoding="utf-8")
        self.tarball = self.root / "upstream.tgz"
        self.tarball.write_bytes(self.upstream_bytes(UPSTREAM))

    @staticmethod
    def upstream_bytes(document):
        return tar_bytes({"package/package.json": b"{}\n", "package/data/table.json": json.dumps(document, indent=2).encode()})

    def upstream(self, tarball=None, **changes):
        tarball = tarball or self.tarball
        options = {"--upstream-tarball": tarball, "--upstream-integrity": integrity(tarball.read_bytes()),
                   "--upstream-member": "package/data/table.json", "--upstream-label": "zqtable@1.2.3",
                   "--accepted-by-upstream": UPSTREAM_WHO}
        options.update(changes)
        return [item for pair in options.items() if pair[1] is not None for item in pair]

    def arguments(self, member, *options, rule="zqclass-1", who=WHO, archive=None, path=MEMBER):
        self.count += 1
        base = self.root / f"case-{self.count}"
        base.mkdir()
        self.member = member
        self.archive, self.out = base / "payload.tar.gz", base / "entry.json"
        self.archive.write_bytes(product_bytes(member) if archive is None else archive)
        return ["--archive", self.archive, "--member", path, "--names", self.names, "--rule", rule, "--out", self.out,
                *(["--accepted-by-identifier", who] if who is not None else []), *options]

    def prove(self, member, *options, **settings):
        return run(TOOL, *self.arguments(member, *options, **settings))

    def entry(self):
        return json.loads(self.out.read_text())

    def only(self, name, count=1, **others):
        return {**{key: 0 for key in CLASS_FIXTURES}, name: count, **others}


class CodeIdentifierEntry(Fixture):
    def test_bare_identifiers_are_proved_and_the_entry_names_the_scan_rule_and_the_member(self):
        done = self.prove(IDENTIFIERS)
        self.assertEqual(done.returncode, 0, done.stderr)
        digest = hashlib.sha256(IDENTIFIERS).hexdigest()
        self.assertEqual(self.entry(), {
            "category": "zqclass", "rule": "zqclass-1", "sha256": digest,
            "reason": "COINCIDENTAL-CODE-IDENTIFIER: bare code identifier equal to the token alone (2 letter-case forms), "
                      "4 of 4, none inside a string literal, template literal, comment, regex literal or property name",
            "accepted_by": "COINCIDENTAL-CODE-IDENTIFIER: " + WHO,
            "proof": {"tool": "release-code-identifier-entry", "hits": 4,
                      "census": self.only("COINCIDENTAL-CODE-IDENTIFIER", 4),
                      "classes": {"COINCIDENTAL-CODE-IDENTIFIER": {"count": 4, "letter_case_forms": 2}}}})
        self.assertEqual(self.out.stat().st_mode & 0o777, 0o600)
        self.assertIn(f"member sha256: {digest}\nhits of rule zqclass-1: 4\n", done.stdout)
        self.assertEqual(census(done.stdout), self.only("COINCIDENTAL-CODE-IDENTIFIER", 4))
        self.assertNotEqual(digest, hashlib.sha256(self.archive.read_bytes()).hexdigest())

    def test_a_hit_in_a_string_comment_template_regex_or_name_position_is_not_a_code_identifier(self):
        for name, members in CLASS_FIXTURES.items():
            if name in ("COINCIDENTAL-CODE-IDENTIFIER", "PUBLIC-UPSTREAM-CONTENT"):
                continue
            for member in members:
                done = self.prove(member)
                self.assertEqual(done.returncode, 1, (name, member))
                self.assertEqual(census(done.stdout), self.only(name), (name, member))
                self.assertIn("PROOF FAILED", done.stdout)
                self.assertFalse(self.out.exists(), (name, member))

    def test_one_hit_outside_the_accepted_classes_refuses_the_whole_member(self):
        done = self.prove(IDENTIFIERS + b"// zqfixturename\n")
        self.assertEqual(done.returncode, 1)
        self.assertEqual(census(done.stdout), self.only("COINCIDENTAL-CODE-IDENTIFIER", 4, COMMENT=1))
        self.assertFalse(self.out.exists())
        done = self.prove(b"var nothing=1;\n")
        self.assertEqual((done.returncode, census(done.stdout), self.out.exists()), (1, self.only("COMMENT", 0), False))

    def test_a_string_element_equal_to_pinned_upstream_content_is_proved(self):
        done = self.prove(DATABASE, *self.upstream())
        self.assertEqual(done.returncode, 0, done.stderr)
        pinned = integrity(self.tarball.read_bytes())
        self.assertEqual(self.entry(), {
            "category": "zqclass", "rule": "zqclass-1", "sha256": hashlib.sha256(DATABASE).hexdigest(),
            "reason": "PUBLIC-UPSTREAM-CONTENT: string literal equal to the token alone, 1 of 1, an array element whose "
                      "key-value element and enclosing entry are equal after whitespace and quote normalisation to an "
                      f"element of zqtable@1.2.3:package/data/table.json (tarball {pinned})",
            "accepted_by": "PUBLIC-UPSTREAM-CONTENT: " + UPSTREAM_WHO,
            "proof": {"tool": "release-code-identifier-entry", "hits": 1, "census": self.only("PUBLIC-UPSTREAM-CONTENT"),
                      "classes": {"PUBLIC-UPSTREAM-CONTENT": {"count": 1, "upstream": {
                          "label": "zqtable@1.2.3", "member": "package/data/table.json", "integrity": pinned}}}}})
        self.assertIn("  string hit: array element; array of 2 element(s); key-value element KEY:[...] equal to an "
                      "upstream element: yes; enclosing entry KEY2:{...} with 2 field(s), equal to an upstream entry: yes\n",
                      done.stdout)
        # the same member with code identifiers too: one entry, both classes, each with its own acceptance text
        done = self.prove(DATABASE + IDENTIFIERS, *self.upstream())
        self.assertEqual(done.returncode, 0, done.stderr)
        entry = self.entry()
        self.assertEqual(entry["accepted_by"],
                         f"COINCIDENTAL-CODE-IDENTIFIER: {WHO} || PUBLIC-UPSTREAM-CONTENT: {UPSTREAM_WHO}")
        self.assertEqual(entry["proof"]["census"], self.only("PUBLIC-UPSTREAM-CONTENT", 1, **{"COINCIDENTAL-CODE-IDENTIFIER": 4}))
        self.assertEqual(sorted(entry["proof"]["classes"]), ["COINCIDENTAL-CODE-IDENTIFIER", "PUBLIC-UPSTREAM-CONTENT"])

    def test_a_string_is_not_proved_without_equal_upstream_content(self):
        other_list = dict(UPSTREAM, zqkey={"zqlist": ["zqplain"], "flag": True})
        other_entry = dict(UPSTREAM, zqkey={"zqlist": ["zqplain", "zqfixturename"], "flag": False})
        for document, fact in ((other_list, "equal to an upstream element: NO"), (other_entry, "equal to an upstream entry: NO")):
            tarball = self.root / f"other-{self.count}.tgz"
            tarball.write_bytes(self.upstream_bytes(document))
            done = self.prove(DATABASE, *self.upstream(tarball))
            self.assertEqual(done.returncode, 1, fact)
            self.assertEqual(census(done.stdout), self.only("STRING-LITERAL-NOT-PROVED"))
            self.assertIn(fact, done.stdout)
            self.assertFalse(self.out.exists())
        # a literal that holds more than the token, or that is no array element, is not this class
        # even when the upstream content is the same
        wider = dict(UPSTREAM, zqkey={"zqlist": ["zqplain", "zqfixturename x"], "flag": True})
        scalar = dict(UPSTREAM, zqkey={"zqlist": "zqfixturename", "flag": True})
        for document, member in ((wider, DATABASE.replace(b"'zqfixturename'", b"'zqfixturename x'")),
                                 (scalar, DATABASE.replace(b"[\"zqplain\",'zqfixturename']", b"'zqfixturename'"))):
            self.assertNotEqual(member, DATABASE)
            tarball = self.root / f"same-{self.count}.tgz"
            tarball.write_bytes(self.upstream_bytes(document))
            done = self.prove(member, *self.upstream(tarball))
            self.assertEqual((done.returncode, census(done.stdout)), (1, self.only("STRING-LITERAL-NOT-PROVED")))
            self.assertFalse(self.out.exists())
        # without the upstream arguments nothing can be proved about a string
        done = self.prove(DATABASE)
        self.assertEqual((done.returncode, census(done.stdout)), (1, self.only("STRING-LITERAL-NOT-PROVED")))
        self.assertFalse(self.out.exists())

    def test_a_wrong_upstream_integrity_or_a_missing_upstream_member_refuses(self):
        wrong = integrity(self.tarball.read_bytes() + b"\x00")
        done = self.prove(DATABASE, *self.upstream(**{"--upstream-integrity": wrong}))
        self.assertEqual(done.returncode, 1)
        self.assertEqual(done.stdout, "PROOF FAILED: the upstream tarball does not have the given sha512 integrity; "
                                      "nothing written\n")
        self.assertFalse(self.out.exists())
        done = self.prove(DATABASE, *self.upstream(**{"--upstream-member": "package/absent.json"}))
        self.assertEqual(done.returncode, 1)
        self.assertEqual(done.stdout, "PROOF FAILED: the upstream member is not in the tarball; nothing written\n")
        self.assertFalse(self.out.exists())

    def test_the_member_is_found_through_tar_and_zip_boundaries_and_its_sha256_is_the_entry_s(self):
        member = CLASS_FIXTURES["COINCIDENTAL-CODE-IDENTIFIER"][0]
        nested = tar_bytes({"pkg/wheels/library.whl": zip_bytes({"library/static/app.js": member, "library/other.js": b"1\n"})})
        for archive, path in ((nested, "pkg/wheels/library.whl!library/static/app.js"), (nested, "library.whl!app.js"),
                              (zip_bytes({"static/app.js": member}), "static/app.js")):
            done = self.prove(member, archive=archive, path=path)
            self.assertEqual(done.returncode, 0, (path, done.stderr))
            self.assertEqual(self.entry()["sha256"], hashlib.sha256(member).hexdigest())
        # a member path that names no member, or more than one, is a tool failure: nothing is written
        twice = zip_bytes({"a/app.js": member, "b/app.js": member})
        for archive, path in ((nested, "library.whl!absent.js"), (twice, "app.js")):
            done = self.prove(member, archive=archive, path=path)
            self.assertEqual((done.returncode, done.stdout), (3, ""))
            self.assertRegex(done.stderr, r"\Arelease-code-identifier-entry: failed with LookupError at "
                                          r"release-code-identifier-entry\.py:\d+(, release-code-identifier-entry\.py:\d+)*\n\Z")
            self.assertFalse(self.out.exists())

    def test_no_output_ever_holds_the_token(self):
        runs = [self.prove(DATABASE + IDENTIFIERS, *self.upstream()), ]
        produced = [self.out.read_text()]
        runs.append(self.prove(DATABASE + IDENTIFIERS + b"var o={zqfixturename:'zqfixturename'};// zqfixturename\n"))
        runs.append(self.prove(IDENTIFIERS, path="zqfixturename.js"))
        runs.append(self.prove(IDENTIFIERS, who="for zqfixturename"))
        self.assertEqual([done.returncode for done in runs], [0, 1, 3, 2])
        for done in runs:
            produced += [done.stdout, done.stderr]
        for text in produced:
            self.assertNotIn(TOKEN, text.lower())
        self.assertIn("string hit: array element; array of 2 element(s)", runs[0].stdout)

    def test_no_key_or_line_of_any_output_names_a_size(self):
        proved = self.prove(DATABASE + IDENTIFIERS, *self.upstream())
        entry = self.entry()
        refused = self.prove(DATABASE.replace(b"'zqfixturename'", b"'zqfixturename x'") + b"var o={zqfixturename:1};\n"
                             + b"var zqfixturename2=`zqfixturename`;// zqfixturename\nvar r=/zqfixturename/;\n", *self.upstream())
        self.assertEqual((proved.returncode, refused.returncode), (0, 1))
        self.assertEqual(census(refused.stdout), {**{name: 1 for name in CLASS_FIXTURES}, "COINCIDENTAL-CODE-IDENTIFIER": 0,
                                                  "PUBLIC-UPSTREAM-CONTENT": 0})
        keys = set(keys_of(entry))
        self.assertEqual(keys, {"category", "rule", "sha256", "reason", "accepted_by", "proof", "tool", "hits", "census",
                                "classes", "count", "letter_case_forms", "upstream", "label", "member", "integrity"})
        for key in keys:
            self.assertFalse(SIZE_WORDS & set(re.split(r"[^a-z]+", key.lower())), key)
        for text in (proved.stdout, refused.stdout, proved.stderr, refused.stderr, json.dumps(entry)):
            self.assertIsNone(re.search(r"(?i)\b(length|size|width|span|longer than|shorter than)\b|\d\s*bytes?\b", text), text)

    def test_usage_errors_exit_2_and_write_nothing(self):
        cases = {
            "no-identifier-text": lambda: self.arguments(IDENTIFIERS, who=None),
            "blank-identifier-text": lambda: self.arguments(IDENTIFIERS, who="  "),
            "two-line-text": lambda: self.arguments(IDENTIFIERS, who="zq\nreviewer"),
            "text-holds-the-token": lambda: self.arguments(IDENTIFIERS, who="ok for ZqFixtureName"),
            "label-holds-the-token": lambda: self.arguments(DATABASE, *self.upstream(**{"--upstream-label": "zqfixturename@1"})),
            "bad-label": lambda: self.arguments(DATABASE, *self.upstream(**{"--upstream-label": "zqtable"})),
            "rule-not-in-the-list": lambda: self.arguments(IDENTIFIERS, rule="zqclass-2"),
            "no-rule": lambda: [arg for arg in self.arguments(IDENTIFIERS) if arg not in ("--rule", "zqclass-1")],
            "review-rule": lambda: self.arguments(b"var a=zqreviewed;\n", rule="review-zqword-1"),
            "bad-names": lambda: self.arguments(IDENTIFIERS)[:5] + [self.archive] + self.arguments(IDENTIFIERS)[6:],
        }
        for missing in ("--upstream-tarball", "--upstream-integrity", "--upstream-member", "--upstream-label",
                        "--accepted-by-upstream"):
            cases["no " + missing] = lambda missing=missing: self.arguments(DATABASE, *self.upstream(**{missing: None}))
        cases["blank --accepted-by-upstream"] = lambda: self.arguments(
            DATABASE, *self.upstream(**{"--accepted-by-upstream": " "}))
        for label, arguments in cases.items():
            done = run(TOOL, *arguments())
            self.assertEqual(done.returncode, 2, (label, done.stdout, done.stderr))
            self.assertNotIn(TOKEN, (done.stdout + done.stderr).lower(), label)
            self.assertFalse(self.out.exists(), label)
        arguments = self.arguments(IDENTIFIERS)
        self.out.write_text("kept\n")
        done = run(TOOL, *arguments)
        self.assertEqual((done.returncode, self.out.read_text()), (2, "kept\n"))


class ClassTable(Fixture):
    """The class table drives the classification, the census, the entry decision and the proof."""

    def patched(self):
        return load(f"code_identifier_entry_{self.count}_{id(self)}", TOOL)

    def test_every_row_has_a_fixture_that_reaches_it(self):
        tool = self.patched()
        self.assertEqual(set(tool.CLASSES), set(CLASS_FIXTURES))
        for name, members in CLASS_FIXTURES.items():
            options = self.upstream() if name == "PUBLIC-UPSTREAM-CONTENT" else []
            done = self.prove(members[0], *options)
            self.assertEqual(census(done.stdout), self.only(name), name)
            self.assertEqual((done.returncode, self.out.exists()), (0, True) if tool.CLASSES[name]["accepting"] else (1, False), name)
            if self.out.exists():
                entry = self.entry()
                self.assertEqual(set(entry), {"category", "rule", "sha256", "reason", "accepted_by", "proof"})
                self.assertEqual(set(entry["proof"]), {"tool", "hits", "census", "classes"})
                self.assertEqual({key: set(value) for key, value in entry["proof"]["classes"].items()},
                                 {name: set(tool.CLASSES[name]["proof"])})
                self.assertTrue(entry["accepted_by"].startswith(name + ": zq "))
                self.assertTrue(entry["reason"].startswith(name + ": "))

    def test_a_row_marked_not_accepting_writes_no_entry_even_with_its_acceptance_text(self):
        member = CLASS_FIXTURES["COINCIDENTAL-CODE-IDENTIFIER"][0]
        tool = self.patched()
        code, _out, err = call(tool, *self.arguments(member))
        self.assertEqual((code, self.out.exists()), (0, True), err)
        tool.CLASSES["COINCIDENTAL-CODE-IDENTIFIER"]["accepting"] = False
        code, out, _err = call(tool, *self.arguments(member))
        self.assertEqual((code, self.out.exists()), (1, False))
        self.assertEqual(census(out), self.only("COINCIDENTAL-CODE-IDENTIFIER"))
        # and a row marked accepting is accepted, with the acceptance text its row names
        tool.CLASSES["COMMENT"].update(accepting=True, accepted_by="accepted_by_zq_comment",
                                       proof=("count",), reason=lambda fields, total: "%d of %d" % (fields["count"], total))
        code, _out, _err = call(tool, *self.arguments(CLASS_FIXTURES["COMMENT"][0]))
        self.assertEqual((code, self.out.exists()), (1, False))
        code, _out, err = call(tool, *self.arguments(CLASS_FIXTURES["COMMENT"][0], "--accepted-by-zq-comment", "zq owner"))
        self.assertEqual(code, 0, err)
        self.assertEqual((self.entry()["accepted_by"], self.entry()["reason"]), ("COMMENT: zq owner", "COMMENT: 1 of 1"))

    def test_an_entry_lacking_a_proof_field_of_its_row_is_not_written(self):
        member = CLASS_FIXTURES["COINCIDENTAL-CODE-IDENTIFIER"][0]
        tool = self.patched()
        tool.CLASSES["COINCIDENTAL-CODE-IDENTIFIER"]["proof"] = ("count", "letter_case_forms", "zqfield")
        code, out, _err = call(tool, *self.arguments(member))
        self.assertEqual((code, self.out.exists()), (1, False))
        self.assertIn("PROOF FAILED: the proof field zqfield that COINCIDENTAL-CODE-IDENTIFIER needs is missing", out)
        tool.PROOF_FIELDS["zqfield"] = lambda mine, context: None
        code, _out, _err = call(tool, *self.arguments(member))
        self.assertEqual((code, self.out.exists()), (1, False))
        tool.PROOF_FIELDS["zqfield"] = lambda mine, context: 7 * len(mine)
        code, _out, err = call(tool, *self.arguments(member))
        self.assertEqual(code, 0, err)
        self.assertEqual(self.entry()["proof"]["classes"],
                         {"COINCIDENTAL-CODE-IDENTIFIER": {"count": 1, "letter_case_forms": 1, "zqfield": 7}})
        # the upstream row needs the pinned upstream as a proof field: without one it is not written
        tool = self.patched()
        tool.CLASSES["COINCIDENTAL-CODE-IDENTIFIER"]["proof"] = ("count", "upstream")
        code, out, _err = call(tool, *self.arguments(member))
        self.assertEqual((code, self.out.exists()), (1, False))
        self.assertIn("the proof field upstream that COINCIDENTAL-CODE-IDENTIFIER needs is missing", out)

    def test_a_new_class_is_one_row_and_its_test(self):
        member = b"var a=ZQFIXTURENAME;var b=zqfixturename;\n"
        tool = self.patched()
        code, out, _err = call(tool, *self.arguments(member))
        self.assertEqual((code, census(out)), (0, self.only("COINCIDENTAL-CODE-IDENTIFIER", 2)))
        tool.CLASSES["ZQ-UPPER-CASE"] = {
            "label": "made-up class", "tests": ((5, lambda h: h["region"] is None and h["form"].isupper()),),
            "accepting": True, "accepted_by": "accepted_by_zq_upper_case", "required": False, "arguments": (),
            "proof": ("count",), "reason": lambda fields, total: "upper case, %d of %d" % (fields["count"], total)}
        code, out, _err = call(tool, *self.arguments(member))
        self.assertEqual((code, self.out.exists()), (1, False))
        self.assertEqual(census(out), {**self.only("COINCIDENTAL-CODE-IDENTIFIER"), "ZQ-UPPER-CASE": 1})
        self.assertIn("PROOF FAILED: no acceptance text was given for ZQ-UPPER-CASE", out)
        code, out, err = call(tool, *self.arguments(member, "--accepted-by-zq-upper-case", "zq owner"))
        self.assertEqual(code, 0, err)
        entry = self.entry()
        self.assertEqual(entry["accepted_by"], "COINCIDENTAL-CODE-IDENTIFIER: zq reviewer || ZQ-UPPER-CASE: zq owner")
        self.assertEqual(entry["proof"]["census"], {**self.only("COINCIDENTAL-CODE-IDENTIFIER"), "ZQ-UPPER-CASE": 1})
        self.assertEqual(entry["proof"]["classes"], {"COINCIDENTAL-CODE-IDENTIFIER": {"count": 1, "letter_case_forms": 1},
                                                     "ZQ-UPPER-CASE": {"count": 1}})
        self.assertTrue(entry["reason"].endswith(" || ZQ-UPPER-CASE: upper case, 1 of 2"))
        # the order of the tests is the table's: after the identifier test the new class takes nothing
        tool.CLASSES["ZQ-UPPER-CASE"]["tests"] = ((70, tool.CLASSES["ZQ-UPPER-CASE"]["tests"][0][1]),)
        code, out, _err = call(tool, *self.arguments(member))
        self.assertEqual((code, census(out)), (0, {**self.only("COINCIDENTAL-CODE-IDENTIFIER", 2), "ZQ-UPPER-CASE": 0}))

    def test_a_hit_no_row_claims_and_two_tests_of_one_order_fail_closed(self):
        member = CLASS_FIXTURES["PROPERTY-NAME"][0]
        tool = self.patched()
        del tool.CLASSES["PROPERTY-NAME"]
        code, out, err = call(tool, *self.arguments(member))
        self.assertEqual((code, out, self.out.exists()), (3, "", False))
        self.assertIn("failed with Failure at", err)
        tool = self.patched()
        tool.CLASSES["COMMENT"]["tests"] = ((60, tool.in_comment),)
        code, out, err = call(tool, *self.arguments(CLASS_FIXTURES["COINCIDENTAL-CODE-IDENTIFIER"][0]))
        self.assertEqual((code, out, self.out.exists()), (3, "", False))

    def test_an_entry_whose_own_text_would_hold_the_token_is_not_written(self):
        member = CLASS_FIXTURES["COINCIDENTAL-CODE-IDENTIFIER"][0]
        tool = self.patched()
        tool.CLASSES["COINCIDENTAL-CODE-IDENTIFIER"]["reason"] = lambda fields, total: "about ZqFixtureName"
        code, out, err = call(tool, *self.arguments(member))
        self.assertEqual((code, self.out.exists()), (2, False))
        self.assertNotIn(TOKEN, (out + err).lower())


class EntryFedToTheScan(Fixture):
    """The entry this tool writes is an acceptance the payload scan itself takes: with it the scan accepts
    the member's hits, and it refuses without it, for a member of other bytes, and for the same entry
    without who accepted it."""

    SCAN = HERE / "release-payload-privacy-scan.py"
    CONVERTER = HERE / "privacy-rules-from-names.py"

    def scan(self, payload, entries):
        """A proof-only run of the payload scan on the product archive: (exit code, report or None)."""
        self.count += 1
        base = self.root / f"scan-{self.count}"
        base.mkdir()
        rules, exemptions, report = base / "rules.json", base / "exemptions.json", base / "report.json"
        done = run(self.CONVERTER, "--names", self.names, "--rules", rules, "--exemptions", exemptions)
        self.assertEqual(done.returncode, 0, done.stderr)
        args = ["--rules", rules, "--exemptions", exemptions, "--report", report, "--detail", base / "detail.json",
                "--work", base, "--proof-only"]
        if entries is not None:
            accepted = base / "accepted.json"
            schema = re.search(r'^SCHEMA_ACCEPTED = "([^"]+)"', self.SCAN.read_text(), re.M).group(1)
            accepted.write_text(json.dumps({"schema": schema, "accepted": entries}))
            args += ["--accepted-findings", accepted]
        done = run(self.SCAN, *args, payload)
        self.assertNotIn(TOKEN, (done.stdout + done.stderr).lower())
        return done.returncode, json.loads(report.read_text()) if report.exists() else None

    def test_the_scan_accepts_the_members_hits_with_the_entry_and_refuses_without_it(self):
        member = b"var a=zqfixturename;\nvar b=zqfixturename+1;\n"
        self.assertEqual(self.prove(member).returncode, 0)
        entry, payload = self.entry(), self.archive
        category = entry["category"]
        # Without the entry the scan refuses the member's hits.
        code, report = self.scan(payload, None)
        self.assertEqual((code, report["status"]), (1, "REFUSED"))
        self.assertEqual((set(report["hits"]["refused"]), report["hits"]["accepted"]), ({category}, {}))
        refused = report["hits"]["refused"][category]
        # With it the same hits are accepted, and nothing is refused.
        code, report = self.scan(payload, [entry])
        self.assertEqual((code, report["status"], report["hits"]["refused"]), (0, "PASS", {}))
        self.assertEqual(report["hits"]["accepted"], {category: refused})
        self.assertEqual(report["accepted"]["count"], 1)
        # The entry binds the member's bytes: one more hit in the member, and the scan refuses again.
        self.assertEqual(self.prove(member + b"var c=zqfixturename;\n", who=None).returncode, 2)
        code, report = self.scan(self.archive, [entry])
        self.assertEqual((code, report["status"]), (1, "REFUSED"))
        self.assertEqual((set(report["hits"]["refused"]), report["hits"]["accepted"]), ({category}, {}))
        # The validator wants who accepted it and a proof that is an object.
        for broken in ({key: value for key, value in entry.items() if key != "accepted_by"}, {**entry, "accepted_by": ""},
                       {**entry, "proof": "because"}, {**entry, "rule": entry["rule"] + "-other"}):
            code, report = self.scan(payload, [broken])
            if broken.get("rule") != entry["rule"]:
                self.assertEqual((code, report["status"], report["hits"]["accepted"]), (1, "REFUSED", {}))
            else:
                self.assertEqual((code, report), (4, None), sorted(broken))

    def test_a_member_the_tool_refuses_leaves_the_scan_refusing(self):
        # A hit in a string is not proved: no entry is written, so the scan has nothing to accept.
        self.assertEqual(self.prove(b'var a=zqfixturename;\nvar s="zqfixturename";\n').returncode, 1)
        self.assertFalse(self.out.exists())
        code, report = self.scan(self.archive, None)
        self.assertEqual((code, report["status"]), (1, "REFUSED"))


class ConverterForms(unittest.TestCase):
    """The tool reads the name list through the converter beside it. The converter's reader returns the
    rows, or the rows with the list's digest: the tool reads both forms."""

    def test_the_name_list_is_read_through_either_form_of_the_converter(self):
        root = Path(tempfile.mkdtemp(prefix="identifier-entry-converter-")).resolve()
        self.addCleanup(shutil.rmtree, root, True)
        converter = (HERE / "privacy-rules-from-names.py").read_text()
        names = root / "names.txt"
        names.write_text("zqclass\tzqfixtureword\n")
        forms = {"rows": "    return value[0] if isinstance(value, tuple) else value\n",
                 "rows-and-digest": "    return value if isinstance(value, tuple) else (value, '0' * 64)\n"}
        for form, body in forms.items():
            directory = root / form
            directory.mkdir()
            shutil.copyfile(TOOL, directory / TOOL.name)
            (directory / "privacy-rules-from-names.py").write_text(
                converter + "\n\n_read_names = read_names\n\n\ndef read_names(path):\n    value = _read_names(path)\n" + body)
            done = subprocess.run([sys.executable, "-I", str(directory / TOOL.name), "--archive", str(root / "absent.tgz"),
                                   "--member", "package/a.js", "--names", str(names), "--rule", "no-such-rule",
                                   "--out", str(root / (form + ".json")), "--accepted-by-identifier", "the test"],
                                  capture_output=True, text=True)
            # The list was read and its rules were built: the refusal is about the rule id, not the reader.
            self.assertEqual(done.returncode, 2, (form, done.stderr))
            self.assertIn("the rule is not in the name list", done.stderr, form)


if __name__ == "__main__":
    unittest.main()
