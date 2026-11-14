"""Rules written from a name list, and their use by the release payload scan.

Synthetic names only: every token below is made up, and no real list, rule or release byte is used.
"""
from pathlib import Path
import hashlib
import importlib.util
import json
import shutil
import subprocess
import sys
import tempfile
import unittest

HERE = Path(__file__).resolve().parent
CONVERTER = HERE / "privacy-rules-from-names.py"
WRAPPER = HERE / "release-payload-privacy-scan.py"
ENGINE = HERE / "release-evidence-privacy-scan.py"
NAMES = "# a comment\nproduct\tzqalpha\nproduct\tzqbeta7\nroster\tZqGamma\nreview-word\tzqdelta\nbus\tzq-epsilon\n"
# Classes whose longer names match inside other words, and names on both sides of the length threshold.
SUBSTRING_NAMES = ("product\tzqomega\nproduct\tzqphi\nprivate_link\tzqlinkhost\nprivate_link\tzqurl\n"
                   "roster\tzqsigmaname\nreview-product\tzqreviewname\n")


def load(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    value = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(value)
    return value


def run(script, *args):
    return subprocess.run([sys.executable, "-I", str(script), *map(str, args)], capture_output=True, text=True)


class RulesFromNames(unittest.TestCase):
    def setUp(self):
        self.root = Path(tempfile.mkdtemp(prefix="rules-from-names-test-")).resolve()
        self.addCleanup(shutil.rmtree, self.root, True)
        self.count = 0

    def convert(self, names=NAMES, *options):
        self.count += 1
        base = self.root / f"case-{self.count}"
        base.mkdir()
        (base / "names.txt").write_text(names, encoding="utf-8")
        done = run(CONVERTER, "--names", base / "names.txt", "--rules", base / "rules.json",
                   "--exemptions", base / "exemptions.json", *options)
        return done, base

    def hits(self, base, text):
        """Category -> count, by the engine itself with the written rules."""
        engine = load("engine_for_rules_test", ENGINE)
        rules = engine.Rules(json.loads((base / "rules.json").read_text()),
                             json.loads((base / "exemptions.json").read_text()))
        (base / "sample.txt").write_text(text, encoding="utf-8")
        report, detail = base / "report.json", base / "detail.json"
        for path in (report, detail):
            if path.exists():
                path.unlink()
        self.assertGreater(rules.rule_count, 0)
        run(ENGINE, "--rules", base / "rules.json", "--exemptions", base / "exemptions.json",
            "--report", report, "--detail", detail, base / "sample.txt")
        found = {}
        for hit in json.loads(detail.read_text())["hits"]:
            found[hit["category"]] = found.get(hit["category"], 0) + int(hit["count"])
        return found

    def test_classes_become_categories_and_tokens_ranked_rules(self):
        done, base = self.convert()
        self.assertEqual(done.returncode, 0, done.stderr)
        rules = json.loads((base / "rules.json").read_text())
        self.assertEqual(rules["schema"], "release-evidence-privacy-scan/rules/1")
        self.assertEqual({name: [rule["id"] for rule in body["rules"]] for name, body in rules["categories"].items()},
                         {"product": ["product-1", "product-2"], "roster": ["roster-1"], "bus": ["bus-1"],
                          "review_word": ["review-word-1"], "home_paths": ["home-paths"]})
        self.assertEqual(json.loads((base / "exemptions.json").read_text()),
                         {"schema": "release-evidence-privacy-scan/exemptions/1", "exemptions": []})
        for name in ("rules.json", "exemptions.json"):
            self.assertEqual((base / name).stat().st_mode & 0o777, 0o600)

    def test_the_output_names_counts_and_never_a_token(self):
        done, base = self.convert()
        printed = done.stdout + done.stderr
        for token in ("zqalpha", "zqbeta7", "zqgamma", "zqdelta", "zq-epsilon"):
            self.assertNotIn(token, printed.lower())
        self.assertEqual(json.loads(done.stdout)["categories"],
                         {"bus": 1, "home_paths": 1, "product": 2, "review_word": 1, "roster": 1})

    def test_a_token_matches_as_the_name_list_scanner_matches_it(self):
        done, base = self.convert()
        self.assertEqual(done.returncode, 0, done.stderr)
        # the boundary rule (every class but the substring classes): any case; a digit or a hyphen may
        # follow a token that ends in a letter; a letter may not
        self.assertEqual(self.hits(base, "x ZQGAMMA y, zqgamma-2, zqgamma9\n"), {"roster": 3})
        self.assertEqual(self.hits(base, "zqgammas and azqgamma and 9zqgamma\n"), {})
        # a token that ends in a digit needs no digit after it
        done, digits = self.convert("roster\tzqbeta7\n")
        self.assertEqual(self.hits(digits, "zqbeta7. zqbeta77 zqbeta7x\n"), {"roster": 1})
        self.assertEqual(self.hits(base, "by zqgamma and zq-epsilon\n"), {"roster": 1, "bus": 1})
        self.assertEqual(self.hits(base, "see /home/someone/work and /Users/other/x\n"), {"home_paths": 2})

    def test_a_longer_product_or_link_name_matches_inside_other_words(self):
        converter = load("converter_for_substring_test", CONVERTER)
        self.assertEqual(converter.SUBSTRING_MATCH, (frozenset({"product", "private_link"}), 6))
        done, base = self.convert(SUBSTRING_NAMES)
        self.assertEqual(done.returncode, 0, done.stderr)
        # a name of 6 or more characters in a substring class: plural, compound and camel-case forms
        self.assertEqual(self.hits(base, "ZqOmegaClient and myzqomegas, zqomega_tool, 9zqomega\n"), {"product": 4})
        self.assertEqual(self.hits(base, "see https://xzqlinkhost.example/ and ZQLINKHOSTS\n"), {"private_link": 2})
        # a shorter name of the same classes keeps the boundary rule
        self.assertEqual(self.hits(base, "zqphis and azqphi and ZqPhiClient, zqurls\n"), {})
        self.assertEqual(self.hits(base, "the zqphi tool at zqurl\n"), {"product": 1, "private_link": 1})
        # every other class keeps the boundary rule whatever the length, a review form of the class too
        self.assertEqual(self.hits(base, "zqsigmanames and azqsigmaname, zqreviewnames\n"), {})
        self.assertEqual(self.hits(base, "zqsigmaname and zqreviewname\n"), {"roster": 1, "review_product": 1})
        # the threshold itself: 5 characters keep the boundary, 6 match inside a word
        done, base = self.convert("product\tzqfiv\nproduct\tzqsixx\n")
        self.assertEqual(self.hits(base, "azqfivs\n"), {})
        self.assertEqual(self.hits(base, "azqsixxs\n"), {"product": 1})
        # the class is the one its category names: another spelling of it is the same class
        done, base = self.convert("Private-Link\tzqlinkhost\n")
        self.assertEqual(self.hits(base, "xzqlinkhosts\n"), {"private_link": 1})

    def test_the_rules_name_the_digest_of_their_name_list(self):
        done, base = self.convert()
        rules = json.loads((base / "rules.json").read_text())
        self.assertEqual(rules["name_list_sha256"], hashlib.sha256((base / "names.txt").read_bytes()).hexdigest())
        self.assertNotIn(rules["name_list_sha256"], done.stdout + done.stderr)

    def test_a_class_name_the_engine_would_not_take_as_a_rule_id_is_still_usable(self):
        done, base = self.convert("-odd class\tzqoddname\n")
        self.assertEqual(done.returncode, 0, done.stderr)
        self.assertEqual(self.hits(base, "an zqoddname here\n"), {"c__odd_class": 1})

    def test_review_classes_and_the_path_category_can_be_review_only(self):
        done, base = self.convert(NAMES, "--review-class", "bus", "--paths", "review")
        self.assertEqual(done.returncode, 0, done.stderr)
        categories = json.loads((base / "rules.json").read_text())["categories"]
        self.assertEqual(sorted(categories), ["product", "review_bus", "review_home_paths", "review_word", "roster"])
        done, base = self.convert(NAMES, "--paths", "off")
        self.assertNotIn("home_paths", json.loads((base / "rules.json").read_text())["categories"])

    def test_bad_input_writes_nothing(self):
        for names, options in (("product zqalpha\n", ()), ("# only a comment\n", ()),
                               (NAMES, ("--review-class", "absent"))):
            done, base = self.convert(names, *options)
            self.assertEqual(done.returncode, 2)
            self.assertFalse((base / "rules.json").exists() or (base / "exemptions.json").exists())
        done, base = self.convert()
        again = run(CONVERTER, "--names", base / "names.txt", "--rules", base / "rules.json",
                    "--exemptions", base / "other.json")
        self.assertEqual(again.returncode, 2)

    def test_the_payload_scan_refuses_a_listed_token_and_only_counts_a_review_class(self):
        payloads = load("payload_helpers_for_rules_test", HERE / "test_release_payload_privacy_scan.py")
        done, base = self.convert()
        self.assertEqual(done.returncode, 0, done.stderr)

        def scan(text, name):
            product = base / name
            product.write_bytes(payloads.product_bytes(
                {"consumers/demo-0.1-py3-none-any.whl": payloads.wheel_bytes({"demo/GUIDE.md": text})}))
            report = base / (name + ".report.json")
            done = run(WRAPPER, "--proof-only", "--rules", base / "rules.json", "--exemptions", base / "exemptions.json",
                       "--report", report, "--detail", base / (name + ".detail.json"), "--work", base, product)
            return done.returncode, json.loads(report.read_text())

        code, report = scan(b"made with ZqAlphaClient\n", "blocking.tar.gz")
        self.assertEqual((code, report["status"], report["hits"]["refused"]), (1, "REFUSED", {"product": 1}))
        code, report = scan(b"a zqdelta here\n", "review.tar.gz")
        self.assertEqual((code, report["status"], report["hits"]), (0, "PASS", {"accepted": {}, "refused": {}}))
        self.assertEqual(report["review_hits"], {"review_word": 1})
        self.assertEqual(report["name_list_sha256"], hashlib.sha256((base / "names.txt").read_bytes()).hexdigest())
        self.assertEqual(report["rules"]["total"], 6)
        self.assertEqual(report["rules"]["blocking"], 5)

    def test_the_report_tells_a_review_class_from_the_review_path_category(self):
        # A release's rules are written with --paths review and no --review-class: every rule of a
        # name class is blocking. The scan's report shows when that is not so, whichever way a review
        # class came to be, and the two counts alone do not (the last case has them as the first).
        payloads = load("payload_helpers_for_review_count_test", HERE / "test_release_payload_privacy_scan.py")
        product = self.root / "counted.tar.gz"
        product.write_bytes(payloads.product_bytes({"selected/notes.txt": b"public notes\n"}))
        blocking = "product\tzqalpha\nproduct\tzqbeta7\nroster\tZqGamma\n"
        for label, names, options, expected in (
                ("the release call", blocking, ("--paths", "review"), (4, 3, 0)),
                ("blocking paths", blocking, (), (4, 4, 0)),
                ("a review class by option", blocking, ("--paths", "review", "--review-class", "roster"), (4, 2, 1)),
                ("a review class by name", NAMES, ("--paths", "review"), (6, 4, 1)),
                ("a review class and blocking paths", blocking + "review-word\tzqdelta\n", ("--review-class", "roster"), (5, 3, 2)),
                ("one review name and blocking paths", blocking, ("--review-class", "roster"), (4, 3, 1))):
            with self.subTest(rules=label):
                done, base = self.convert(names, *options)
                self.assertEqual(done.returncode, 0, done.stderr)
                report = base / "report.json"
                done = run(WRAPPER, "--proof-only", "--rules", base / "rules.json", "--exemptions", base / "exemptions.json",
                           "--report", report, "--detail", base / "detail.json", "--work", base, product)
                value = json.loads(report.read_text())
                self.assertEqual((done.returncode, value["status"]), (0, "PASS"))
                self.assertEqual(value["rules"], {"sha256": hashlib.sha256((base / "rules.json").read_bytes()).hexdigest(),
                                                  "total": expected[0], "blocking": expected[1], "review_outside_paths": expected[2]})

    def test_no_class_of_names_takes_a_name_of_the_path_category(self):
        # Both names of the built-in category are its own, whichever one the call asks for.
        for name in ("home_paths", "home-paths", "Home Paths", "review-home-paths", "review_home_paths", "REVIEW-HOME_PATHS"):
            for paths in ("block", "review", "off"):
                with self.subTest(name=name, paths=paths):
                    done, base = self.convert(NAMES + name + "\tzqmadeupname\n", "--paths", paths)
                    self.assertEqual(done.returncode, 2, done.stdout)
                    self.assertFalse((base / "rules.json").exists() or (base / "exemptions.json").exists())
                    self.assertNotIn("zqmadeupname", done.stdout + done.stderr)
        # A review class by option under the category's plain name is the same thing.
        done, base = self.convert(NAMES + "home_paths\tzqmadeupname\n", "--paths", "off", "--review-class", "home_paths")
        self.assertEqual((done.returncode, (base / "rules.json").exists()), (2, False))

    def test_the_payload_scan_cannot_pass_with_rules_that_refuse_nothing(self):
        payloads = load("payload_helpers_for_vacuous_test", HERE / "test_release_payload_privacy_scan.py")
        product = self.root / "vacuous.tar.gz"
        product.write_bytes(payloads.product_bytes({"selected/notes.txt": b"a zqdelta here\n"}))
        for label, names, options, reason in (
                ("review-only", "review-word\tzqdelta\n", ("--paths", "off"), "no-blocking-rules"),
                ("paths-off", NAMES, ("--paths", "off"), "home-paths-off")):
            with self.subTest(rules=label):
                done, base = self.convert(names, *options)
                self.assertEqual(done.returncode, 0, done.stderr)
                report = base / "report.json"
                done = run(WRAPPER, "--proof-only", "--rules", base / "rules.json", "--exemptions",
                           base / "exemptions.json", "--report", report, "--detail", base / "detail.json",
                           "--work", base, product)
                value = json.loads(report.read_text())
                self.assertEqual((done.returncode, value["status"]), (2, "REFUSED"))
                self.assertIn(reason, value["reasons"])


if __name__ == "__main__":
    unittest.main()
