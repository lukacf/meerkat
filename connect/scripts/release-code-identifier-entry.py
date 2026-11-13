#!/usr/bin/env python3
"""Re-prove, for ONE name-list rule and ONE JavaScript member of a payload archive, that every hit is
of a stated harmless class, and only then write ONE accepted-findings entry for
release-payload-privacy-scan.py. The proof fails closed.

  python3 -I release-code-identifier-entry.py --archive ARCHIVE --member PATH[!PATH...] \\
      --names LIST [--review-class CLASS ...] --rule RULE_ID --out NEW_FILE \\
      --accepted-by-identifier TEXT \\
      [--upstream-tarball TGZ --upstream-integrity sha512-... --upstream-member package/data/table.json \\
       --upstream-label NAME@VERSION --accepted-by-upstream TEXT]

The name list is private and never committed (`CLASS<TAB>TOKEN` lines). It is read by
privacy-rules-from-names.py beside this file, loaded as a module (its command line is never run),
which gives the token of --rule the category and pattern the payload scan uses. --review-class is
passed to it unchanged. The rule is named only on the command line, with the payload scan's rule
id; there is no built-in rule. --member follows `!` boundaries through nested tar and zip archives
in memory. Hits: the rule's pattern over the bytes of the member decoded as latin-1.

Classes. The table CLASSES below is the one place that names them: each row holds the class's test
with its order, whether the class can be accepted, the option that carries its acceptance text,
the other arguments it needs and the proof fields the entry must hold for it. A hit gets the class
of the first test, in ascending order, that claims it; a hit no test claims is a tool failure.

COINCIDENTAL-CODE-IDENTIFIER (tokenizer pass over the member, in memory): the hit is in code - not
inside a string literal, a template literal's text, a comment or a regex literal - is an identifier
token equal to the token alone (any letter case), and is not a property name (not directly after
"." or "?.", and not an object-literal key: previous significant character "{" or "," and next
significant character ":").

PUBLIC-UPSTREAM-CONTENT (only with the --upstream-* arguments; no network): the hit is a string
literal whose content is the token alone, an element of an array; the smallest enclosing key-value
element KEY:[...] and, when there is one, the enclosing entry KEY2:{...} are converted from
JavaScript literal text to JSON (whitespace dropped, quotes normalised, bare keys quoted, !0/!1
read as true/false) and must be structurally equal to an element of the JSON member of the pinned
upstream tarball, whose sha512 integrity is verified before it is read.

Every other class (a string literal that is not proved, template literal text, a comment, a regex
literal, a property name, part of a longer identifier) is never accepted: nothing is written,
exit 1, counts only.

Entry (--out, a new file, mode 0600: one JSON object for the `accepted` list of a
release-payload-privacy-scan.py accepted-findings document), written only when the rule has hits, every hit
is in a class the table accepts, the acceptance text of each such class was given on the command
line and every proof field the table names for the class is present:
  {"category": C, "rule": R, "sha256": sha256 of the member file, "reason": the counts per class
   and what was proved, "accepted_by": "CLASS: TEXT" for each class, joined by " || ",
   "proof": {"tool": this tool, "hits": N, "census": {CLASS: count, for every class of the table},
             "classes": {CLASS: {proof field: value}}}}
The tool has no approver of its own: every acceptance text comes from the command line.

Never printed, written or logged: the token, its length or a fact that bounds it (no size, width
or end of a hit, of the literal that holds it or of the element around it), matched bytes, keys,
elements or any text of the member or of the upstream file. An unexpected failure prints the
exception type and its file:line, nothing else.
Exit status: 0 entry written; 1 proof failed; 2 usage error; 3 tool failure.
Python 3.11 or later, standard library only, no network.
"""
import argparse
import base64
import bisect
import collections
import hashlib
import io
import json
import os
import pathlib
import re
import sys
import tarfile
import traceback
import types
import zipfile

sys.dont_write_bytecode = True
TOOL = "release-code-identifier-entry"
HERE = os.path.dirname(os.path.abspath(__file__))
CONVERTER = os.path.join(HERE, "privacy-rules-from-names.py")
REVIEW_PREFIX = "review_"
IDENT_START = set("abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ_$")
IDENT_PART = IDENT_START | set("0123456789")
REGEX_PREV_KEYWORDS = {"return", "typeof", "instanceof", "in", "of", "new", "delete", "void", "throw", "case", "do",
                       "else", "yield", "await"}
REGEX_PREV_PUNCT = set("(,=:[!&|?{};+-*%<>~^")
WS = " \t\r\n"
LABEL_RX = re.compile(r"[@A-Za-z0-9_./-]{1,80}@[A-Za-z0-9_.+-]{1,40}")


class Usage(Exception):
    pass


class Failure(Exception):
    pass


def load_rules(names, review_classes):
    """[(category, rule id, compiled pattern)] for every token of the name list, as the converter
    beside this file builds them for the payload scan. The list stays in memory."""
    with open(CONVERTER, "rb") as stream:
        raw = stream.read()
    module = types.ModuleType("privacy_rules_from_names")
    module.__file__ = CONVERTER
    exec(compile(raw, CONVERTER, "exec", dont_inherit=True), module.__dict__)
    try:
        rows = module.read_names(pathlib.Path(names))
        if isinstance(rows, tuple):     # a converter that also returns the list's digest: (rows, sha256)
            rows = rows[0]
        document = module.build(rows, set(review_classes), "off")
    except module.Refused:
        raise Usage("the name list or a --review-class was refused; privacy-rules-from-names.py names the reason") from None
    rules = []
    for category, body in document["categories"].items():
        for rule in body["rules"]:
            if rule.get("kind") != "regex" or not isinstance(rule.get("value"), str):
                raise Failure("a rule of the name list is not a pattern")
            rules.append((category, rule["id"], re.compile(rule["value"], re.IGNORECASE if rule.get("ignore_case") else 0)))
    return rules


def read_member(archive, member):
    """Follow PATH!PATH!... through nested tar/zip archives in memory. Returns bytes."""
    with open(archive, "rb") as f:
        blob = f.read()
    for seg in member.split("!"):
        found = None
        if blob[:4] == b"PK\x03\x04":
            with zipfile.ZipFile(io.BytesIO(blob)) as zf:
                names = [n for n in zf.namelist() if not n.endswith("/")]
                hits = [n for n in names if n == seg] or [n for n in names if n.endswith("/" + seg)]
                if len(hits) == 1:
                    found = zf.read(hits[0])
        else:
            with tarfile.open(fileobj=io.BytesIO(blob), mode="r:*") as tf:
                mems = [m for m in tf if m.isreg()]
                hits = [m for m in mems if m.name == seg] or [m for m in mems if m.name.endswith("/" + seg)]
                if len(hits) == 1:
                    found = tf.extractfile(hits[0]).read()
        if found is None:
            raise LookupError("member segment not found or not unique")
        blob = found
    return blob


def tokenize(text):
    """Non-code regions as (start, end, kind): 's' string, 't' template text, 'c' comment, 'r' regex."""
    regions = []
    n = len(text)
    i = 0
    prev_sig = ""
    prev_word = ""
    tmpl_stack = []
    while i < n:
        c = text[i]
        if c in WS:
            i += 1
            continue
        if c == "/" and i + 1 < n and text[i + 1] == "/":
            j = text.find("\n", i)
            j = n if j < 0 else j
            regions.append((i, j, "c"))
            i = j
            continue
        if c == "/" and i + 1 < n and text[i + 1] == "*":
            j = text.find("*/", i + 2)
            j = n if j < 0 else j + 2
            regions.append((i, j, "c"))
            i = j
            continue
        if c in "\"'":
            j = i + 1
            while j < n and text[j] != c:
                if text[j] == "\\":
                    j += 1
                elif text[j] == "\n":
                    break
                j += 1
            j = min(n, j + 1)
            regions.append((i, j, "s"))
            prev_sig, prev_word = c, ""
            i = j
            continue
        if c == "`" or (c == "}" and tmpl_stack and tmpl_stack[-1] == 0):
            if c == "}":
                tmpl_stack.pop()
            j = i + 1
            closed = False
            while j < n:
                d = text[j]
                if d == "\\":
                    j += 2
                    continue
                if d == "`":
                    closed = True
                    break
                if d == "$" and j + 1 < n and text[j + 1] == "{":
                    break
                j += 1
            if closed or j >= n:
                regions.append((i, min(n, j + 1), "t"))
                prev_sig, prev_word = "`", ""
                i = j + 1
            else:
                regions.append((i, j + 2, "t"))
                tmpl_stack.append(0)
                prev_sig, prev_word = "{", ""
                i = j + 2
            continue
        if c == "{" and tmpl_stack:
            tmpl_stack[-1] += 1
        elif c == "}" and tmpl_stack:
            tmpl_stack[-1] -= 1
        if c == "/":
            regex_ok = (prev_sig == "" or prev_sig in REGEX_PREV_PUNCT) if not prev_word else (prev_word in REGEX_PREV_KEYWORDS)
            if regex_ok:
                j = i + 1
                in_class = False
                ok = False
                while j < n and text[j] != "\n":
                    d = text[j]
                    if d == "\\":
                        j += 2
                        continue
                    if d == "[":
                        in_class = True
                    elif d == "]":
                        in_class = False
                    elif d == "/" and not in_class:
                        ok = True
                        break
                    j += 1
                if ok:
                    j += 1
                    while j < n and text[j] in IDENT_PART:
                        j += 1
                    regions.append((i, j, "r"))
                    prev_sig, prev_word = ")", ""
                    i = j
                    continue
        if c in IDENT_START:
            j = i + 1
            while j < n and text[j] in IDENT_PART:
                j += 1
            prev_word = text[i:j]
            prev_sig = "a"
            i = j
            continue
        if c.isdigit():
            j = i + 1
            while j < n and (text[j] in IDENT_PART or text[j] == "."):
                j += 1
            prev_sig, prev_word = "0", ""
            i = j
            continue
        prev_sig, prev_word = c, ""
        i += 1
    return regions


class Code:
    """Bracket navigation over code, skipping the non-code regions."""

    def __init__(self, text, regions):
        self.text, self.regions = text, regions
        self.starts = [r[0] for r in regions]

    def region_at(self, pos):
        k = bisect.bisect_right(self.starts, pos) - 1
        if k >= 0 and pos < self.regions[k][1]:
            return self.regions[k]
        return None

    def find_open(self, pos):
        depth = 0
        p = pos - 1
        while p >= 0:
            r = self.region_at(p)
            if r:
                p = r[0] - 1
                continue
            ch = self.text[p]
            if ch in ")]}":
                depth += 1
            elif ch in "([{":
                if depth == 0:
                    return p
                depth -= 1
            p -= 1
        return -1

    def find_close(self, open_pos):
        depth = 0
        p = open_pos + 1
        n = len(self.text)
        while p < n:
            r = self.region_at(p)
            if r:
                p = r[1]
                continue
            ch = self.text[p]
            if ch in "([{":
                depth += 1
            elif ch in ")]}":
                if depth == 0:
                    return p
                depth -= 1
            p += 1
        return -1

    def key_before(self, pos):
        """A key token that ends right before the ':' preceding pos. Returns (key_start, key_text) or None."""
        p = pos - 1
        while p >= 0 and self.text[p] in WS:
            p -= 1
        if p < 0 or self.text[p] != ":":
            return None
        p -= 1
        while p >= 0 and self.text[p] in WS:
            p -= 1
        if p < 0:
            return None
        r = self.region_at(p)
        if r and r[2] == "s" and r[1] == p + 1:
            return r[0], json.loads(js_to_json(self.text[r[0]:r[1]]))
        if self.text[p] in IDENT_PART:
            q = p
            while q >= 0 and self.text[q] in IDENT_PART:
                q -= 1
            return q + 1, self.text[q + 1:p + 1]
        return None


def js_to_json(src):
    """JavaScript literal text (objects, arrays, strings, numbers, !0/!1, true/false/null) to JSON text."""
    out = []
    i, n = 0, len(src)
    while i < n:
        c = src[i]
        if c in WS:
            i += 1
        elif c in "\"'":
            j = i + 1
            buf = []
            while j < n and src[j] != c:
                if src[j] == "\\" and j + 1 < n:
                    e = src[j + 1]
                    if e == "u" and j + 5 < n:
                        buf.append(chr(int(src[j + 2:j + 6], 16)))
                        j += 6
                        continue
                    if e == "x" and j + 3 < n:
                        buf.append(chr(int(src[j + 2:j + 4], 16)))
                        j += 4
                        continue
                    buf.append({"n": "\n", "t": "\t", "r": "\r", "b": "\b", "f": "\f", "0": "\0"}.get(e, e))
                    j += 2
                    continue
                buf.append(src[j])
                j += 1
            if j >= n:
                raise ValueError("unterminated string")
            out.append(json.dumps("".join(buf)))
            i = j + 1
        elif c in "{}[],:":
            out.append(c)
            i += 1
        elif c == "!" and src[i + 1:i + 2] in ("0", "1"):
            out.append("true" if src[i + 1] == "0" else "false")
            i += 2
        elif c.isdigit() or c in "-.":
            m = re.match(r"-?(?:\d+\.?\d*|\.\d+)(?:[eE][+-]?\d+)?", src[i:])
            if not m:
                raise ValueError("not a literal")
            num = m.group(0)
            out.append(("0" + num) if num.startswith(".") else (num.replace("-.", "-0.") if num.startswith("-.") else num.rstrip(".")))
            i += len(num)
        elif c in IDENT_START:
            j = i + 1
            while j < n and src[j] in IDENT_PART:
                j += 1
            word = src[i:j]
            k = j
            while k < n and src[k] in WS:
                k += 1
            if k < n and src[k] == ":":
                out.append(json.dumps(word))
            elif word in ("true", "false", "null"):
                out.append(word)
            else:
                raise ValueError("not a literal")
            i = j
        else:
            raise ValueError("not a literal")
    return "".join(out)


def json_has(doc, key, value):
    """Is there a mapping anywhere in doc with doc[key] == value?"""
    stack = [doc]
    while stack:
        cur = stack.pop()
        if isinstance(cur, dict):
            if key in cur and cur[key] == value:
                return True
            stack.extend(cur.values())
        elif isinstance(cur, list):
            stack.extend(cur)
    return False


def string_proof(code, upstream, region, matched):
    """(proved, fact) for a hit inside the string literal `region`. The fact holds counts only."""
    text = code.text
    proved = False
    fact = "string literal"
    try:
        if text[region[0] + 1:region[1] - 1].lower() != matched.lower():
            raise ValueError("literal is longer than the token")
        op = code.find_open(region[0])
        if op < 0 or text[op] != "[":
            raise ValueError("not an array element")
        cl = code.find_close(op)
        arr = json.loads(js_to_json(text[op:cl + 1]))
        k1 = code.key_before(op)
        fact = "array element; array of %d element(s)" % len(arr)
        if upstream is None or k1 is None:
            raise ValueError("no key or no upstream")
        l1 = json_has(upstream, k1[1], arr)
        fact += "; key-value element KEY:[...] equal to an upstream element: %s" % ("yes" if l1 else "NO")
        l2 = None
        op2 = code.find_open(k1[0])
        if op2 >= 0 and text[op2] == "{":
            k2 = code.key_before(op2)
            if k2 is not None:
                obj = json.loads(js_to_json(text[op2:code.find_close(op2) + 1]))
                l2 = json_has(upstream, k2[1], obj)
                fact += "; enclosing entry KEY2:{...} with %d field(s), equal to an upstream entry: %s" % (
                    len(obj), "yes" if l2 else "NO")
        proved = l1 and l2 is not False
    except (ValueError, IndexError, KeyError):
        fact += "; element not comparable"
    return proved, fact


def hit_facts(code, upstream, s, e):
    """What the tests read about one hit. It holds text of the member: memory only, never written."""
    text = code.text
    region = code.region_at(s)
    h = {"region": region[2] if region else None, "proved": False, "fact": None, "form": text[s:e],
         "longer": False, "property": False}
    if region:
        if region[2] == "s":
            h["proved"], h["fact"] = string_proof(code, upstream, region, text[s:e])
        return h
    h["longer"] = (s > 0 and text[s - 1] in IDENT_PART) or (e < len(text) and text[e] in IDENT_PART)
    p = s - 1
    while p >= 0 and text[p] in WS:
        p -= 1
    q = e
    while q < len(text) and text[q] in WS:
        q += 1
    prev_c = text[p] if p >= 0 else ""
    next_c = text[q] if q < len(text) else ""
    # An empty prev_c (the hit starts the member) counts as a key position, as "{" and "," do.
    h["property"] = prev_c == "." or (prev_c in "{," and next_c == ":")
    return h


# ---- the tests: each says whether it claims the hit ----

def in_template_text(h):
    return h["region"] == "t"


def in_comment(h):
    return h["region"] == "c"


def in_regex_literal(h):
    return h["region"] == "r"


def proved_upstream_string(h):
    return h["region"] == "s" and h["proved"]


def other_string(h):
    return h["region"] == "s"


def longer_identifier(h):
    return h["region"] is None and h["longer"]


def property_name(h):
    return h["region"] is None and h["property"]


def bare_identifier(h):
    return h["region"] is None and not h["longer"] and not h["property"]


# ---- the proof fields: each returns the value for one class's hits, or None ----

def proof_count(mine, context):
    return len(mine) or None


def proof_letter_case_forms(mine, context):
    return len({h["form"] for h in mine}) or None


def proof_upstream(mine, context):
    return context.get("upstream")


PROOF_FIELDS = {"count": proof_count, "letter_case_forms": proof_letter_case_forms, "upstream": proof_upstream}


def identifier_reason(fields, total):
    forms = fields["letter_case_forms"]
    return ("bare code identifier equal to the token alone (%d letter-case form%s), %d of %d, none inside a string "
            "literal, template literal, comment, regex literal or property name" % (
                forms, "" if forms == 1 else "s", fields["count"], total))


def upstream_reason(fields, total):
    up = fields["upstream"]
    return ("string literal equal to the token alone, %d of %d, an array element whose key-value element and "
            "enclosing entry are equal after whitespace and quote normalisation to an element of %s:%s (tarball %s)" % (
                fields["count"], total, up["label"], up["member"], up["integrity"]))


# ---- the class table. A new class is one row here, its test function above and its test. ----
#  label        what the count line says.
#  tests        (order, test) pairs; a hit gets the class of the first test, by ascending order over
#               all rows, that claims it.
#  accepting    whether the entry may be written when the rule has hits of this class.
#  accepted_by  the option that carries the acceptance text (None for a class never accepted).
#  required     the acceptance text must be given on every run (a usage error without it).
#  arguments    the other options the class needs: all of them and the acceptance text, or none.
#  proof        the proof fields (keys of PROOF_FIELDS) the entry must hold for this class.
#  reason       the function that writes the class's part of the entry's reason from those fields.
def never(label, order, test):
    return {"label": label, "tests": ((order, test),), "accepting": False, "accepted_by": None, "required": False,
            "arguments": (), "proof": (), "reason": None}


CLASSES = collections.OrderedDict([
    ("COINCIDENTAL-CODE-IDENTIFIER", {
        "label": "code identifier equal to the token alone", "tests": ((60, bare_identifier),),
        "accepting": True, "accepted_by": "accepted_by_identifier", "required": True, "arguments": (),
        "proof": ("count", "letter_case_forms"), "reason": identifier_reason}),
    ("PUBLIC-UPSTREAM-CONTENT", {
        "label": "string literal equal to the token alone, proved upstream element",
        "tests": ((20, proved_upstream_string),),
        "accepting": True, "accepted_by": "accepted_by_upstream", "required": False,
        "arguments": ("upstream_tarball", "upstream_integrity", "upstream_member", "upstream_label"),
        "proof": ("count", "upstream"), "reason": upstream_reason}),
    ("STRING-LITERAL-NOT-PROVED", never("string literal, not proved", 30, other_string)),
    ("TEMPLATE-LITERAL-TEXT", never("template literal text", 10, in_template_text)),
    ("COMMENT", never("comment", 11, in_comment)),
    ("REGEX-LITERAL", never("regex literal", 12, in_regex_literal)),
    ("PROPERTY-NAME", never("property name", 50, property_name)),
    ("LONGER-IDENTIFIER", never("part of a longer identifier", 40, longer_identifier)),
])


def ordered_tests():
    """Every (order, class, test) of the table, ascending; two tests may not share an order."""
    tests = sorted(((order, name, test) for name, row in CLASSES.items() for order, test in row["tests"]),
                   key=lambda item: item[0])
    if len({order for order, _name, _test in tests}) != len(tests):
        raise Failure("two tests of the class table share one order")
    return tests


def option(attribute):
    return "--" + attribute.replace("_", "-")


def main(argv=None):
    tests = ordered_tests()
    ap = argparse.ArgumentParser(description="One accepted-findings entry for one rule and one JavaScript member.",
                                 allow_abbrev=False)
    for name in ("archive", "member", "names", "rule", "out"):
        ap.add_argument("--" + name, required=True)
    ap.add_argument("--review-class", action="append", default=[], metavar="CLASS",
                    help="as given to privacy-rules-from-names.py")
    declared = set()
    for row in CLASSES.values():
        for attribute in row["arguments"] + ((row["accepted_by"],) if row["accepted_by"] else ()):
            if attribute not in declared:
                declared.add(attribute)
                ap.add_argument(option(attribute))
    a = ap.parse_args(argv)
    os.umask(0o077)
    acceptance = {}
    for name, row in CLASSES.items():
        text = (getattr(a, row["accepted_by"]) or "").strip() if row["accepted_by"] else ""
        if "\n" in text or "\r" in text:
            raise Usage("an acceptance text must be one line")
        group = [getattr(a, attribute) for attribute in row["arguments"]] + ([text] if row["arguments"] else [])
        if row["required"] and not text:
            raise Usage("%s needs a non-empty text" % option(row["accepted_by"]))
        if any(group) and not all(group):
            raise Usage("%s: all or none of %s" % (name, ", ".join(
                option(attribute) for attribute in row["arguments"] + (row["accepted_by"],))))
        acceptance[name] = text
    if os.path.lexists(a.out):
        raise Usage("--out must be a new file")
    if a.upstream_label and not LABEL_RX.fullmatch(a.upstream_label):
        raise Usage("--upstream-label must be NAME@VERSION")
    matches = [(category, rx) for category, rule_id, rx in load_rules(a.names, a.review_class) if rule_id == a.rule]
    if len(matches) != 1:
        raise Usage("the rule is not in the name list" if not matches else "the rule id names more than one category")
    category, rx = matches[0]
    if category.startswith(REVIEW_PREFIX):
        raise Usage("the rule is in a review category: the payload scan counts its hits and accepts none")
    # Checked before anything is printed: the label and member name are echoed below.
    if any(rx.search(given) for given in [*acceptance.values(), a.upstream_label or "", a.upstream_member or ""]):
        raise Usage("an acceptance text or label holds the token")
    upstream = None
    context = {}
    if a.upstream_tarball:
        with open(a.upstream_tarball, "rb") as f:
            blob = f.read()
        integrity = "sha512-" + base64.b64encode(hashlib.sha512(blob).digest()).decode()
        if integrity != a.upstream_integrity:
            print("PROOF FAILED: the upstream tarball does not have the given sha512 integrity; nothing written")
            return 1
        with tarfile.open(fileobj=io.BytesIO(blob), mode="r:*") as tf:
            for mem in tf:
                if mem.isreg() and mem.name == a.upstream_member:
                    upstream = json.loads(tf.extractfile(mem).read().decode("utf-8"))
        if upstream is None:
            print("PROOF FAILED: the upstream member is not in the tarball; nothing written")
            return 1
        context["upstream"] = {"label": a.upstream_label, "member": a.upstream_member, "integrity": integrity}
    data = read_member(a.archive, a.member)
    sha = hashlib.sha256(data).hexdigest()
    text = data.decode("latin-1")
    code = Code(text, tokenize(text))
    by_class = collections.OrderedDict((name, []) for name in CLASSES)
    facts = []
    total = 0
    for m in rx.finditer(text):
        total += 1
        h = hit_facts(code, upstream, m.start(), m.end())
        if h["fact"] is not None:
            facts.append(h["fact"])
        for _order, name, test in tests:
            if test(h):
                by_class[name].append(h)
                break
        else:
            raise Failure("no test of the class table claims a hit")
    print("member sha256: %s" % sha)
    print("hits of rule %s: %d" % (a.rule, total))
    for name, row in CLASSES.items():
        print("  %s: %d - %s" % (name, len(by_class[name]), row["label"]))
    for fact in facts:
        print("  string hit: %s" % fact)
    if "upstream" in context:
        print("upstream: %s member %s, tarball integrity verified (%s)" % (
            a.upstream_label, a.upstream_member, context["upstream"]["integrity"]))
    if total == 0 or any(by_class[name] and not row["accepting"] for name, row in CLASSES.items()):
        print("PROOF FAILED: a hit is neither a bare code identifier equal to the token alone nor a proved upstream "
              "string element; nothing written")
        return 1
    reasons, signers, proof = [], [], collections.OrderedDict()
    for name, row in CLASSES.items():
        mine = by_class[name]
        if not mine:
            continue
        if not row["accepted_by"] or not acceptance[name]:
            print("PROOF FAILED: no acceptance text was given for %s; nothing written" % name)
            return 1
        fields = collections.OrderedDict()
        for field in row["proof"]:
            value = PROOF_FIELDS[field](mine, context) if field in PROOF_FIELDS else None
            if value is None:
                print("PROOF FAILED: the proof field %s that %s needs is missing; nothing written" % (field, name))
                return 1
            fields[field] = value
        reasons.append("%s: %s" % (name, row["reason"](fields, total)))
        signers.append("%s: %s" % (name, acceptance[name]))
        proof[name] = fields
    entry = {"category": category, "rule": a.rule, "sha256": sha, "reason": " || ".join(reasons),
             "accepted_by": " || ".join(signers),
             "proof": {"tool": TOOL, "hits": total, "census": {name: len(mine) for name, mine in by_class.items()},
                       "classes": proof}}
    if rx.search(json.dumps([entry["reason"], entry["accepted_by"], entry["proof"]], sort_keys=True)):
        raise Usage("an acceptance text or label holds the token")
    descriptor = os.open(a.out, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(descriptor, "w", encoding="utf-8") as f:
        json.dump(entry, f, indent=1, sort_keys=True)
        f.write("\n")
    print("entry written: %s" % os.path.abspath(a.out))
    return 0


def run(argv=None):
    try:
        return main(argv)
    except Usage as error:
        sys.stderr.write("%s: usage error: %s\n" % (TOOL, error))
        return 2
    except SystemExit:
        raise
    except BaseException as exc:  # noqa: BLE001 - never echo exception text (it may hold a token)
        tb = traceback.extract_tb(exc.__traceback__)
        sys.stderr.write("%s: failed with %s at %s\n" % (
            TOOL, type(exc).__name__, ", ".join("%s:%d" % (os.path.basename(fr.filename), fr.lineno) for fr in tb[-3:])))
        return 3


if __name__ == "__main__":
    sys.exit(run())
