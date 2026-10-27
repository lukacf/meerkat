#!/usr/bin/env python3
"""Unit tests for scripts/published_crates_standalone.py (no cargo, no network).

The census and the guard run cargo; what they decide from cargo's output is
pure and pinned here: comment stripping, the guard's findings, the test-name
listing parser, the manifest's workspace-only selection and name parity.
"""

from __future__ import annotations

import importlib.util
import pathlib
import sys
import tempfile
import unittest

SCRIPT = pathlib.Path(__file__).resolve().parent / "published_crates_standalone.py"
spec = importlib.util.spec_from_file_location("published_crates_standalone", SCRIPT)
assert spec is not None and spec.loader is not None
pcs = importlib.util.module_from_spec(spec)
sys.modules["published_crates_standalone"] = pcs
spec.loader.exec_module(pcs)


def kinds(findings):
    return [kind for kind, _line, _excerpt in findings]


class FloorNormalization(unittest.TestCase):
    """The published-floor check's manifests: what `cargo package` would
    write for a product crate built against crates.io."""

    def setUp(self):
        self.root = pathlib.Path("/repo")
        self.crate = self.root / "connect" / "crates" / "app"
        self.kept = {(self.root / "connect" / "crates" / "lib").resolve(): "../lib"}

    def normalize(self, name, spec, workspace_deps=None):
        return pcs.normalized_dependency(name, spec, workspace_deps or {}, self.kept, self.crate, self.root)

    def test_workspace_inheritance_merges_features_and_drops_paths_outside_the_product(self):
        spec = self.normalize("meerkat", {"workspace": True, "features": ["b"]},
                              {"meerkat": {"version": "=0.9.6", "path": "crates/meerkat", "features": ["a"]}})
        self.assertEqual(spec, {"version": "=0.9.6", "features": ["a", "b"]})

    def test_a_product_crate_keeps_its_scratch_path(self):
        spec = self.normalize("lib", {"version": "=0.9.4", "path": "../lib"})
        self.assertEqual(spec, {"version": "=0.9.4", "path": "../lib"})

    def test_a_path_dependency_without_a_version_cannot_be_published(self):
        with self.assertRaises(pcs.UsageError):
            self.normalize("tool", {"path": "../../../crates/tool"})

    def test_an_undeclared_workspace_dependency_is_refused(self):
        with self.assertRaises(pcs.UsageError):
            self.normalize("missing", {"workspace": True})


class FloorResolveProblem(unittest.TestCase):
    def test_an_unmet_requirement_is_named(self):
        stderr = "error: failed to select a version for the requirement `meerkat = \"=0.9.6\"`\ncandidate versions found which didn't match: 0.9.5\n"
        self.assertEqual(pcs.resolve_problem(stderr), 'meerkat = "=0.9.6"')

    def test_an_unpublished_crate_is_named(self):
        stderr = ("    Updating crates.io index\n"
                  "error: no matching package named `meerkat-machine-codegen` found\n"
                  "location searched: crates.io index\n"
                  "required by package `toolkit-catalog v0.9.4 (/ws/toolkit-catalog)`\n")
        self.assertEqual(pcs.resolve_problem(stderr), "meerkat-machine-codegen is not on crates.io")

    def test_any_other_error_reports_cargos_first_error_line(self):
        stderr = "warning: x\nerror: failed to parse manifest\n\nCaused by:\n  bad\n"
        self.assertEqual(pcs.resolve_problem(stderr), "failed to parse manifest")


class StripComments(unittest.TestCase):
    def test_comments_blanked_strings_kept_and_lines_preserved(self):
        src = (
            '//! #[path = "a.rs"] mod x;\n'
            'let s = "// not a comment"; /* block\n'
            ' #[path = "b.rs"] */ let c = \'"\';\n'
            'let r = r#"x // y "#; #[path = "c.rs"] mod y;\n'
        )
        out = pcs.strip_comments(src)
        self.assertEqual(out.count("\n"), src.count("\n"))
        self.assertNotIn('#[path = "a.rs"]', out)
        self.assertNotIn('#[path = "b.rs"]', out)
        self.assertIn('"// not a comment"', out)
        self.assertIn('r#"x // y "#', out)
        self.assertIn('#[path = "c.rs"]', out)

    def test_nested_block_comment(self):
        out = pcs.strip_comments("a /* x /* y */ z */ b")
        self.assertEqual(out.split(), ["a", "b"])


class ScanSource(unittest.TestCase):
    PACKAGED = {"src/lib.rs", "src/help.rs", "embedded/skill.md", "tests/support/mod.rs", "tests/a.rs"}

    def scan(self, relative, text):
        return pcs.scan_source(self.PACKAGED, relative, pcs.strip_comments(text))

    def test_include_of_a_packaged_file_is_clean(self):
        self.assertEqual(self.scan("src/help.rs", 'include_str!("../embedded/skill.md")'), [])

    def test_include_leaving_the_package_is_an_escape(self):
        self.assertEqual(kinds(self.scan("src/help.rs", 'include_str!("../../docs/x.md")')), ["include-escape"])

    def test_include_of_an_unpackaged_file_inside_the_root_is_an_escape(self):
        self.assertEqual(kinds(self.scan("src/help.rs", 'include_bytes!("../tests/fixtures/x.bin")')), ["include-escape"])

    def test_include_from_manifest_dir(self):
        ok = 'include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/embedded/skill.md"))'
        bad = 'include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../x.md"))'
        self.assertEqual(kinds(self.scan("src/lib.rs", ok)), [])
        self.assertEqual(kinds(self.scan("src/lib.rs", bad)), ["include-escape"])

    def test_path_attribute(self):
        self.assertEqual(self.scan("tests/a.rs", '#[path = "support/mod.rs"] mod support;'), [])
        self.assertEqual(
            kinds(self.scan("tests/a.rs", '#[path = "../../../tests/fixtures/live/support.rs"] mod s;')),
            ["path-escape"],
        )

    def test_path_attribute_in_a_doc_comment_is_not_a_finding(self):
        self.assertEqual(self.scan("tests/a.rs", '//! declare `#[path = "../../x.rs"] mod x;`\n'), [])

    def test_manifest_dir_walked_up(self):
        parent = 'let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();'
        dotdot = 'let f = Path::new(env!("CARGO_MANIFEST_DIR")).join("../other/src/x.rs");'
        own = 'let f = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/handles");'
        self.assertEqual(kinds(self.scan("tests/a.rs", parent)), ["manifest-dir-walk"])
        self.assertEqual(kinds(self.scan("tests/a.rs", dotdot)), ["manifest-dir-walk"])
        self.assertEqual(self.scan("tests/a.rs", own), [])

    def test_manifest_dir_statement_ends_at_semicolon(self):
        text = 'let own = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");\nlet up = own.parent();'
        self.assertEqual(self.scan("tests/a.rs", text), [])

    def test_manifest_dir_joined_with_a_constant_walks_where_the_constant_does(self):
        use = 'fn t() { let f = Path::new(env!("CARGO_MANIFEST_DIR")).join(FIXTURE); }\n'
        up = 'const FIXTURE: &str =\n    "../../crates/other/tests/fixtures/x.json";\n' + use
        own = 'const FIXTURE: &str = "tests/fixtures/x.json";\n' + use
        unused = 'const OTHER: &str = "../../x.json";\n' + use
        self.assertEqual(kinds(self.scan("src/lib.rs", up)), ["manifest-dir-walk"])
        self.assertEqual(self.scan("src/lib.rs", own), [])
        self.assertEqual(self.scan("src/lib.rs", unused), [])

    def test_root_helper_and_skip_when_missing(self):
        self.assertEqual(kinds(self.scan("tests/a.rs", "fn workspace_root() -> PathBuf { todo!() }")), ["root-helper"])
        self.assertEqual(
            kinds(self.scan("tests/a.rs", 'eprintln!("Skipping: rkat binary not found (build it)");')),
            ["skip-when-missing"],
        )
        self.assertEqual(self.scan("tests/a.rs", 'eprintln!("Skipping: Nushell not installed");'), [])


    def test_code_the_package_compiles_out_is_not_scanned(self):
        walk = 'let r = Path::new(env!("CARGO_MANIFEST_DIR")).join("../other/fixtures");'
        text = (
            '#[cfg(not(target_arch = "wasm32"))]\n'
            '#[cfg(feature = "workspace-tests")]\n'
            "fn stage() -> Result<PathBuf, Box<dyn Error>> {\n"
            "    let s = \"}\"; let c = '}'; let raw = r#\"}\"#;\n"
            f"    {walk}\n"
            "}\n"
            '#[cfg(all(test, feature = "workspace-tests"))]\n'
            "#[tokio::test]\n"
            f"async fn gated_test() {{ {walk} }}\n"
            '#[cfg(any(feature = "workspace-tests", all(unix, feature = "workspace-tests")))]\n'
            f"mod gated {{ fn up() {{ {walk} }} }}\n"
            '#[cfg(feature = "workspace-tests")]\n'
            "use other::{a, b};\n"
            f"fn kept() {{ {walk} }}\n"
        )
        findings = self.scan("src/lib.rs", text)
        self.assertEqual([(kind, line) for kind, line, _ in findings], [("manifest-dir-walk", 14)])

    def test_a_cfg_that_does_not_need_the_feature_keeps_the_code(self):
        walk = 'let r = Path::new(env!("CARGO_MANIFEST_DIR")).parent();'
        for cfg in (
            'not(feature = "workspace-tests")',
            'any(test, feature = "workspace-tests")',
            'feature = "workspace-tests-extra"',
            "test",
            'all(test, not(feature = "workspace-tests"))',
        ):
            with self.subTest(cfg=cfg):
                text = f"#[cfg({cfg})]\nfn f() {{ {walk} }}\n"
                self.assertEqual(kinds(self.scan("src/lib.rs", text)), ["manifest-dir-walk"])
        statement = f'fn f() {{\n    #[cfg(feature = "workspace-tests")]\n    {walk}\n}}\n'
        self.assertEqual(kinds(self.scan("src/lib.rs", statement)), ["manifest-dir-walk"])

    def test_a_file_its_leading_inner_attribute_compiles_out(self):
        walk = 'fn f() { let r = Path::new(env!("CARGO_MANIFEST_DIR")).parent(); }\n'
        gated = '#![allow(dead_code)]\n#![cfg(all(feature = "workspace-tests", unix))]\n' + walk
        self.assertEqual(self.scan("tests/a.rs", gated), [])
        late = walk + 'mod m { #![cfg(feature = "workspace-tests")] }\n'
        self.assertEqual(kinds(self.scan("tests/a.rs", late)), ["manifest-dir-walk"])

    def test_cfg_requires(self):
        feature = "workspace-tests"
        for predicate, needs in (
            ('feature = "workspace-tests"', True),
            ('all(test, feature = "workspace-tests")', True),
            ('any(feature = "workspace-tests", all(feature = "workspace-tests", unix))', True),
            ('any(test, feature = "workspace-tests")', False),
            ('not(feature = "workspace-tests")', False),
            ("any()", False),
            ('feature = "workspace-tests", test', False),
            ('feature = workspace-tests', False),
        ):
            with self.subTest(predicate=predicate):
                self.assertEqual(pcs.cfg_requires(predicate, feature), needs)


class ParseListing(unittest.TestCase):
    OUTPUT = (
        "   Compiling demo v0.9.4\n"
        "     Running unittests src/lib.rs (target/debug/deps/demo-0a1b)\n"
        "tests::alpha: test\n"
        "tests::beta: test\n"
        "\n"
        "     Running tests/flows.rs (target/debug/deps/flows-2c3d)\n"
        "flows_run: test\n"
        "bench_like: benchmark\n"
        "     Running unittests src/main.rs (target/debug/deps/demo_bin-4e5f)\n"
        "   Doc-tests demo\n"
        "src/lib.rs - Thing (line 12): test\n"
    )

    def test_names_per_target(self):
        names = pcs.parse_listing(self.OUTPUT)
        self.assertEqual(names["src/lib.rs"], {"tests::alpha", "tests::beta"})
        self.assertEqual(names["tests/flows.rs"], {"flows_run"})
        self.assertEqual(names["src/main.rs"], set())
        self.assertEqual(names[pcs.DOC_TARGET], {"src/lib.rs - Thing (line 12)"})


class DocTestNames(unittest.TestCase):
    def test_package_prefix_is_stripped_on_both_sides(self):
        workspace = pcs.parse_listing(
            "   Doc-tests demo\ncrates/demo/src/lib.rs - Thing (line 3): test\n", "crates/demo"
        )
        standalone = pcs.parse_listing("   Doc-tests demo\ndemo-0.9.4/src/lib.rs - Thing (line 3): test\n", "demo-0.9.4")
        self.assertEqual(workspace, standalone)
        self.assertEqual(workspace[pcs.DOC_TARGET], {"src/lib.rs - Thing (line 3)"})


class Parity(unittest.TestCase):
    ENTRY = {
        "excluded": [{"path": "tests/repo_gate.rs", "reason": "reads the repository"}],
        "workspace_only": [
            {"target": "src/lib.rs", "modules": ["resolver::workspace_tests"], "reason": "needs meerkat-runtime"},
            {"target": "src/lib.rs", "tests": ["tests::reads_sibling_fixture"], "reason": "sibling fixture"},
        ],
    }
    WORKSPACE = {
        "src/lib.rs": {
            "tests::a",
            "resolver::workspace_tests::x",
            "resolver::workspace_tests::y",
            "tests::reads_sibling_fixture",
        },
        "tests/repo_gate.rs": {"gate"},
        "tests/kept.rs": {"kept"},
    }

    def test_declared_differences_are_parity(self):
        standalone = {"src/lib.rs": {"tests::a"}, "tests/kept.rs": {"kept"}}
        self.assertEqual(pcs.parity(self.ENTRY, self.WORKSPACE, standalone), [])

    def test_a_lost_test_fails(self):
        standalone = {"src/lib.rs": set(), "tests/kept.rs": {"kept"}}
        self.assertEqual(
            pcs.parity(self.ENTRY, self.WORKSPACE, standalone),
            ["src/lib.rs: tests::a runs in the workspace but not standalone"],
        )

    def test_a_vacuous_target_fails(self):
        # A file gated on a feature only the stripped self dev-dep enabled.
        standalone = {"src/lib.rs": {"tests::a"}, "tests/kept.rs": set()}
        self.assertEqual(
            pcs.parity(self.ENTRY, self.WORKSPACE, standalone),
            ["tests/kept.rs: kept runs in the workspace but not standalone"],
        )

    def test_a_test_only_standalone_runs_fails(self):
        standalone = {"src/lib.rs": {"tests::a", "tests::extra"}, "tests/kept.rs": {"kept"}}
        self.assertEqual(
            pcs.parity(self.ENTRY, self.WORKSPACE, standalone),
            ["src/lib.rs: tests::extra runs standalone but not in the workspace"],
        )

    def test_equal_counts_with_swapped_names_fail(self):
        standalone = {"src/lib.rs": {"tests::b"}, "tests/kept.rs": {"kept"}}
        self.assertEqual(len(pcs.parity(self.ENTRY, self.WORKSPACE, standalone)), 2)

    def test_module_prefix_is_a_path_not_a_string_prefix(self):
        names = {"resolver::workspace_tests_extra::z", "resolver::workspace_tests::x"}
        self.assertEqual(
            pcs.workspace_only_names(self.ENTRY, "src/lib.rs", names), {"resolver::workspace_tests::x"}
        )

    def test_a_stale_declaration_fails(self):
        # The declared test was renamed and the declared module emptied.
        workspace = {"src/lib.rs": {"tests::a", "tests::reads_sibling_fixture_renamed"}, "tests/kept.rs": {"kept"}}
        standalone = {"src/lib.rs": {"tests::a"}, "tests/kept.rs": {"kept"}}
        self.assertEqual(
            pcs.parity(self.ENTRY, workspace, standalone),
            [
                "src/lib.rs: declared workspace-only module resolver::workspace_tests holds no workspace test",
                "src/lib.rs: declared workspace-only test tests::reads_sibling_fixture is not a workspace test",
                "src/lib.rs: tests::reads_sibling_fixture_renamed runs in the workspace but not standalone",
            ],
        )


class LoadManifest(unittest.TestCase):
    def load(self, text):
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp)
            (root / "ci").mkdir()
            (root / "ci" / "standalone-workspace-only.toml").write_text(text)
            return pcs.load_manifest(root)

    def test_grouped_workspace_only_loads(self):
        crates = self.load(
            '[crates.demo]\n'
            '[[crates.demo.workspace_only]]\n'
            'target = "src/lib.rs"\nreason = "needs a cyclic dev-dependency"\n'
            'modules = ["a::tests"]\ntests = ["b::tests::one"]\n'
        )
        self.assertEqual(crates["demo"]["workspace_only"][0]["modules"], ["a::tests"])

    def test_a_workspace_only_entry_needs_a_reason_and_a_name(self):
        with self.assertRaises(pcs.UsageError):
            self.load('[[crates.demo.workspace_only]]\ntarget = "src/lib.rs"\nmodules = ["a"]\n')
        with self.assertRaises(pcs.UsageError):
            self.load('[[crates.demo.workspace_only]]\ntarget = "src/lib.rs"\nreason = "x"\n')


class StandaloneFeatures(unittest.TestCase):
    def test_declared_features(self):
        manifest = {"package": {"metadata": {"meerkat": {"standalone-tests": {"features": ["test-support"]}}}}}
        self.assertEqual(pcs.standalone_features(manifest), ["test-support"])
        self.assertEqual(pcs.standalone_features({"package": {}}), [])


def crate_manifest(standalone=None, features=None, dev=None):
    package = {"name": "demo"}
    if standalone is not None:
        package["metadata"] = {"meerkat": {"standalone-tests": standalone}}
    manifest = {"package": package, "features": features or {}}
    if dev is not None:
        manifest["dev-dependencies"] = dev
    return manifest


class PublishedTestContract(unittest.TestCase):
    def failures(self, manifest, readme=None, packaged=("src/lib.rs",)):
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp)
            packaged = set(packaged)
            if readme is not None:
                (root / "README.md").write_text(readme)
                packaged.add("README.md")
            return pcs.published_test_contract_failures("demo", manifest, root, packaged)

    def test_the_command_carries_features_and_stack(self):
        manifest = crate_manifest({"features": ["a", "b"], "min-stack": 8388608}, {"a": [], "b": []})
        self.assertEqual(pcs.test_command(manifest), "RUST_MIN_STACK=8388608 cargo test --features a,b")
        self.assertEqual(pcs.test_command(crate_manifest({"features": ["a"]}, {"a": []})), "cargo test --features a")
        self.assertEqual(pcs.test_command(crate_manifest({"min-stack": 8388608})), "RUST_MIN_STACK=8388608 cargo test")
        self.assertIsNone(pcs.test_command(crate_manifest()))

    def test_a_plain_crate_needs_no_readme(self):
        self.assertEqual(self.failures(crate_manifest()), [])

    def test_the_readme_prints_the_exact_command(self):
        manifest = crate_manifest({"features": ["a"]}, {"a": []})
        self.assertEqual(self.failures(manifest, "Run `cargo test --features a`.\n"), [])
        self.assertEqual(len(self.failures(manifest, "Run `cargo test`.\n")), 1)
        self.assertEqual(len(self.failures(manifest)), 1)

    def test_declared_features_exist_and_never_enable_workspace_tests(self):
        manifest = crate_manifest({"features": ["missing", "workspace-tests"]}, {"workspace-tests": []},
                                  {"demo": {"path": ".", "features": ["workspace-tests"]}})
        readme = "cargo test --features missing,workspace-tests\n"
        self.assertEqual(len(self.failures(manifest, readme)), 2)

    def test_workspace_tests_needs_the_path_only_self_dev_dependency(self):
        declared = {"workspace-tests": []}
        self.assertEqual(len(self.failures(crate_manifest(None, declared))), 1)
        versioned = {"demo": {"path": ".", "version": "1", "features": ["workspace-tests"]}}
        self.assertEqual(len(self.failures(crate_manifest(None, declared, versioned))), 1)
        path_only = {"demo": {"path": ".", "features": ["workspace-tests"]}}
        self.assertEqual(self.failures(crate_manifest(None, declared, path_only)), [])


UNIT_TEST_RULE = """
rust_test(
    name = "demo_crate_unit_test",
    crate = ":demo_crate",
    crate_features = [
{features}
    ],
    deps = [],
)
"""


class BazelWorkspaceTests(unittest.TestCase):
    def failures(self, declared, build=None):
        manifest = crate_manifest(None, declared)
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp)
            if build is not None:
                (root / "BUILD.bazel").write_text(build)
            return pcs.bazel_workspace_tests_failures("demo-crate", manifest, root)

    def rule(self, *features):
        return UNIT_TEST_RULE.format(features="\n".join(f'        "{f}",' for f in features))

    def test_a_crate_without_the_feature_needs_nothing(self):
        self.assertEqual(self.failures({"a": []}), [])

    def test_the_unit_test_target_turns_the_feature_on(self):
        declared = {"workspace-tests": []}
        self.assertEqual(self.failures(declared, self.rule("test-support", "workspace-tests")), [])
        self.assertEqual(len(self.failures(declared, self.rule("test-support"))), 1)
        self.assertEqual(len(self.failures(declared, "rust_library(name = \"demo_crate\")\n")), 1)
        self.assertEqual(len(self.failures(declared)), 1)

    def test_only_the_named_target_counts(self):
        other = self.rule("workspace-tests").replace("demo_crate_unit_test", "demo_crate_other_test")
        self.assertEqual(len(self.failures({"workspace-tests": []}, other + self.rule("test-support"))), 1)


if __name__ == "__main__":
    unittest.main()
