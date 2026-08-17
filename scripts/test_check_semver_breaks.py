#!/usr/bin/env python3
"""Unit tests for the semver-breaks analyser.

The report fixtures are real `cargo-semver-checks` output (see
`scripts/fixtures/semver-breaks/README.md`), and the "correct changelog" case
runs against this repository's own committed 0.8.23 release notes, so a green
run here means the gate accepts a real declared release and rejects a real
undeclared one rather than a hand-built imitation of both.
"""

from __future__ import annotations

import importlib.util
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path


SCRIPT = Path(__file__).with_name("check_semver_breaks.py")
SPEC = importlib.util.spec_from_file_location("check_semver_breaks", SCRIPT)
assert SPEC and SPEC.loader
gate = importlib.util.module_from_spec(SPEC)
# Register before exec: @dataclass resolves annotations through sys.modules.
sys.modules[SPEC.name] = gate
SPEC.loader.exec_module(gate)

FIXTURES = Path(__file__).with_name("fixtures") / "semver-breaks"
SQLITE_REPORT = (FIXTURES / "report-meerkat-sqlite-0.8.22.txt").read_text(encoding="utf-8")
CLEAN_REPORT = (FIXTURES / "report-clean-two-crates.txt").read_text(encoding="utf-8")
WORKSPACE_REPORT = (FIXTURES / "report-workspace-clean-0.8.23.txt").read_text(encoding="utf-8")
REPO_CHANGELOG = (Path(__file__).resolve().parent.parent / "CHANGELOG.md").read_text(
    encoding="utf-8"
)
# Raw evidence that the dedupe below is dropping real duplicate lines rather
# than the parser silently missing half the report.
SQLITE_REPORT_ITEM_LINES = [
    line
    for line in SQLITE_REPORT.splitlines()
    if line.startswith("  ") and line.rstrip().endswith(tuple("0123456789")) and " in /" in line
]


def section_for(changelog: str, version: str) -> "gate.Section":
    for section in gate.parse_changelog(changelog):
        if section.version == version:
            return section
    raise AssertionError(f"no `## [{version}]` section in changelog")


class ReportParsingTests(unittest.TestCase):
    def test_parses_every_real_lint_shape_once(self) -> None:
        parsed = gate.parse_report(SQLITE_REPORT)
        # The tool prints every "Failed in:" line twice (once per feature
        # configuration it checked): 14 lines, 7 distinct breaks.
        self.assertEqual(len(SQLITE_REPORT_ITEM_LINES), 14)
        self.assertEqual(len(parsed.findings), 7)
        self.assertEqual({f.crate for f in parsed.findings}, {"meerkat-sqlite"})
        self.assertTrue(all(f.structural for f in parsed.findings))
        self.assertEqual(
            {(f.lint_id, f.symbols) for f in parsed.findings},
            {
                ("constructible_struct_adds_field", ("MaintenanceBridgeReport", "refused")),
                ("constructible_struct_adds_field", ("MaintenancePrepareReport", "refused")),
                (
                    "constructible_struct_adds_field",
                    ("SchemaDomain", "bridge_recoverable_versions"),
                ),
                ("derive_trait_impl_removed", ("MaintenanceBridgeReport", "Copy")),
                ("derive_trait_impl_removed", ("MaintenancePrepareReport", "Copy")),
                (
                    "enum_struct_variant_field_added",
                    ("bridgeable", "SqliteStoreError", "UnledgeredDomainObjects"),
                ),
                ("enum_variant_added", ("SqliteStoreError", "WalConversionContended")),
            },
        )

    def test_finding_granularity_is_finer_than_type_name(self) -> None:
        parsed = gate.parse_report(SQLITE_REPORT)
        bridge = [f for f in parsed.findings if "MaintenanceBridgeReport" in f.symbols]
        self.assertEqual(len(bridge), 2)
        self.assertEqual(
            {f.lint_id for f in bridge},
            {"constructible_struct_adds_field", "derive_trait_impl_removed"},
        )

    def test_strips_location_suffix(self) -> None:
        self.assertEqual(
            gate.strip_location("field MaintenanceBridgeReport.refused in /repo/a.rs:435"),
            "field MaintenanceBridgeReport.refused",
        )
        self.assertEqual(
            gate.strip_location("type X no longer derives Copy, in /repo/a.rs:425"),
            "type X no longer derives Copy",
        )

    def test_single_colon_variant_path_splits(self) -> None:
        symbols, structural = gate.extract_symbols(
            "enum_variant_added", "variant SqliteStoreError:WalConversionContended"
        )
        self.assertTrue(structural)
        self.assertEqual(symbols, ("SqliteStoreError", "WalConversionContended"))

    def test_module_path_prefix_is_not_required(self) -> None:
        symbols, _ = gate.extract_symbols(
            "enum_variant_added", "variant meerkat_sqlite::SqliteStoreError:Wal"
        )
        self.assertEqual(symbols, ("SqliteStoreError", "Wal"))

    def test_unknown_lint_falls_back_and_is_flagged(self) -> None:
        symbols, structural = gate.extract_symbols(
            "some_future_lint", "type Widget is now sealed by trait Sealed"
        )
        self.assertFalse(structural)
        self.assertEqual(symbols, ("Widget", "Sealed"))

    def test_clean_report_has_no_findings_but_is_measured(self) -> None:
        parsed = gate.parse_report(CLEAN_REPORT)
        self.assertEqual(parsed.findings, [])
        self.assertEqual(parsed.finished_crates, ["meerkat", "meerkat-agent-build-authority"])
        self.assertEqual(parsed.summary_lines, 2)


class NamingTests(unittest.TestCase):
    """Hole A: reported breaks must be NAMED, not merely accompanied by a heading."""

    def test_real_0_8_23_notes_name_every_reported_meerkat_sqlite_break(self) -> None:
        parsed = gate.parse_report(SQLITE_REPORT)
        errors = gate.check_named(parsed, section_for(REPO_CHANGELOG, "0.8.23"))
        self.assertEqual(errors, [], "\n".join(errors))

    def test_omitting_copy_from_a_real_section_is_red(self) -> None:
        # The sharpest real 0.8.23 hole: the notes name MaintenanceBridgeReport
        # and its new `refused` field, but never name the lost `Copy` derive.
        parsed = gate.parse_report(SQLITE_REPORT)
        section = section_for(REPO_CHANGELOG, "0.8.23")
        without_copy = gate.Section(
            section.heading,
            section.label,
            section.suffix,
            section.body.replace("Copy", "Klone"),
        )
        errors = gate.check_named(parsed, without_copy)
        self.assertEqual(len(errors), 2, "\n".join(errors))
        for error in errors:
            self.assertIn("derive_trait_impl_removed", error)
            self.assertIn("`Copy`", error)
        self.assertTrue(any("MaintenanceBridgeReport" in e for e in errors))
        self.assertTrue(any("MaintenancePrepareReport" in e for e in errors))

    def test_missing_break_is_named_in_the_failure(self) -> None:
        parsed = gate.parse_report(SQLITE_REPORT)
        section = section_for(REPO_CHANGELOG, "0.8.23")
        without_variant = gate.Section(
            section.heading,
            section.label,
            section.suffix,
            section.body.replace("WalConversionContended", "SomethingElse"),
        )
        errors = gate.check_named(parsed, without_variant)
        self.assertEqual(len(errors), 1, "\n".join(errors))
        self.assertIn("WalConversionContended", errors[0])
        self.assertIn("enum_variant_added", errors[0])

    def test_breaking_heading_alone_does_not_declare_anything(self) -> None:
        # Exactly what the old gate accepted: the heading exists, names nothing.
        parsed = gate.parse_report(SQLITE_REPORT)
        section = gate.Section(
            "## [0.8.23] - 2026-08-16",
            "0.8.23",
            " - 2026-08-16",
            "\n### Breaking\n\n- Some things changed.\n",
        )
        errors = gate.check_named(parsed, section)
        self.assertEqual(len(errors), 7, "\n".join(errors))

    def test_declaration_outside_the_breaking_subsection_does_not_count(self) -> None:
        parsed = gate.parse_report(SQLITE_REPORT)
        body = section_for(REPO_CHANGELOG, "0.8.23").body
        moved = body.replace("### Breaking", "### Fixed", 1)
        section = gate.Section("## [0.8.23] - 2026-08-16", "0.8.23", " - 2026-08-16", moved)
        errors = gate.check_named(parsed, section)
        self.assertEqual(len(errors), 1, "\n".join(errors))
        self.assertIn("no `### Breaking` heading", errors[0])

    def test_break_named_under_another_heading_is_still_undeclared(self) -> None:
        parsed = gate.parse_report(SQLITE_REPORT)
        real = section_for(REPO_CHANGELOG, "0.8.23").body
        # Keep the full real declarations, but file them under `### Added` and
        # leave a `### Breaking` heading that names nothing.
        moved = "\n### Breaking\n\n- Nothing to see here.\n\n### Added\n" + real
        section = gate.Section("## [0.8.23] - 2026-08-16", "0.8.23", " - 2026-08-16", moved)
        errors = gate.check_named(parsed, section)
        self.assertEqual(len(errors), 7, "\n".join(errors))

    def test_breaking_body_stops_at_the_next_subsection(self) -> None:
        section = gate.Section(
            "## [9.9.9] - 2026-01-01",
            "9.9.9",
            " - 2026-01-01",
            "\n### Breaking\n\n- Alpha\n\n### Added\n\n- Beta\n",
        )
        body = gate.breaking_body(section)
        self.assertIsNotNone(body)
        self.assertIn("Alpha", body or "")
        self.assertNotIn("Beta", body or "")

    def test_substring_of_a_longer_identifier_does_not_name_it(self) -> None:
        self.assertFalse(gate.names_symbol("`MaintenanceBridgeReportV2` changed", "MaintenanceBridgeReport"))
        self.assertTrue(gate.names_symbol("`MaintenanceBridgeReport` changed", "MaintenanceBridgeReport"))

    def test_a_finding_with_no_nameable_symbol_fails_closed(self) -> None:
        parsed = gate.ReportParse(
            findings=[
                gate.Finding(
                    lint_id="future_lint",
                    crate="meerkat-core",
                    item="function meerkat_core::do_thing changed",
                    symbols=(),
                    structural=False,
                )
            ],
            finished_crates=["meerkat-core"],
            summary_lines=1,
        )
        section = gate.Section(
            "## [9.9.9] - 2026-01-01", "9.9.9", " - 2026-01-01", "\n### Breaking\n\n- Everything.\n"
        )
        errors = gate.check_named(parsed, section)
        self.assertEqual(len(errors), 1)
        self.assertIn("no nameable symbol", errors[0])


CHANGELOG_PREAMBLE = "# Changelog\n\nBlah.\n\n"


def changelog(*sections: str) -> str:
    return CHANGELOG_PREAMBLE + "\n".join(sections)


UNRELEASED_EMPTY = "## [Unreleased]\n\n"
UNRELEASED_POPULATED = "## [Unreleased]\n\n### Breaking\n\n- Something broke.\n\n"
STAMPED_23 = "## [0.8.23] - 2026-08-16\n\n### Breaking\n\n- Something broke.\n\n"
STAMPED_22 = "## [0.8.22] - 2026-08-09\n\n### Added\n\n- A thing.\n\n"


class StampingTests(unittest.TestCase):
    """Hole B: the pending section must be declared AGAINST A VERSION."""

    def test_unreleased_notes_after_the_version_bump_are_red(self) -> None:
        # The real 0.8.23 state: version bumped to 0.8.23, ~420 lines of notes
        # still under `## [Unreleased]`, topmost stamped section still 0.8.22.
        errors = gate.check_stamped(
            gate.parse_changelog(changelog(UNRELEASED_POPULATED, STAMPED_22)), "0.8.23"
        )
        self.assertEqual(len(errors), 1, "\n".join(errors))
        self.assertIn("Unreleased", errors[0])
        self.assertIn("0.8.23", errors[0])

    def test_unreleased_notes_before_the_version_bump_are_green(self) -> None:
        # Preflight runs before `cargo release` bumps: notes for the NEXT
        # release sit under [Unreleased] and the workspace version is still the
        # last stamped release. That is the healthy pre-bump state.
        self.assertEqual(
            gate.check_stamped(
                gate.parse_changelog(changelog(UNRELEASED_POPULATED, STAMPED_23, STAMPED_22)),
                "0.8.23",
            ),
            [],
        )

    def test_empty_unreleased_stub_above_a_stamped_release_is_green(self) -> None:
        # Deliberate preserved behaviour: stamping leaves an empty stub.
        self.assertEqual(
            gate.check_stamped(
                gate.parse_changelog(changelog(UNRELEASED_EMPTY, STAMPED_23, STAMPED_22)),
                "0.8.23",
            ),
            [],
        )

    def test_empty_stub_above_a_stale_stamped_release_is_red(self) -> None:
        errors = gate.check_stamped(
            gate.parse_changelog(changelog(UNRELEASED_EMPTY, STAMPED_22)), "0.8.23"
        )
        self.assertEqual(len(errors), 1, "\n".join(errors))
        self.assertIn("0.8.22", errors[0])

    def test_stamped_section_without_a_date_is_red(self) -> None:
        errors = gate.check_stamped(
            gate.parse_changelog(changelog("## [0.8.23]\n\n- x\n\n", STAMPED_22)), "0.8.23"
        )
        self.assertEqual(len(errors), 1, "\n".join(errors))
        self.assertIn("release date", errors[0])

    # There is deliberately NO test here asserting that the live CHANGELOG is
    # stamped for the live workspace version. This suite is the guard that
    # `check-semver-breaks.sh` runs BEFORE trusting the analyser, and a live
    # repo-state assertion inside it turns the one state the gate exists to
    # catch - version bumped, notes still under `## [Unreleased]` - into
    # "the analyser failed its own unit tests", which is a red for a reason
    # other than the one it names. Verified by simulating that state: with the
    # workspace version at 0.8.24 and the notes unstamped, this suite went red
    # on that test and the gate never reached its own actionable message.
    # The gate itself asserts the live fact, against the real changelog, with
    # the message that says what to do about it.


class MeasuredTests(unittest.TestCase):
    """A report the gate could not produce is not evidence of no breaks."""

    def test_clean_report_with_exit_zero_is_measured(self) -> None:
        parsed = gate.parse_report(CLEAN_REPORT)
        self.assertEqual(gate.check_measured(parsed, 0, None), [])

    def test_nonzero_exit_with_no_findings_is_red(self) -> None:
        parsed = gate.parse_report(CLEAN_REPORT)
        errors = gate.check_measured(parsed, 101, None)
        self.assertEqual(len(errors), 1, "\n".join(errors))
        self.assertIn("could not measure", errors[0])

    def test_zero_exit_with_findings_is_red(self) -> None:
        parsed = gate.parse_report(SQLITE_REPORT)
        errors = gate.check_measured(parsed, 0, None)
        self.assertEqual(len(errors), 1, "\n".join(errors))
        self.assertIn("disagree", errors[0])

    def test_truncated_report_is_red(self) -> None:
        truncated = "\n".join(CLEAN_REPORT.splitlines()[:4])
        parsed = gate.parse_report(truncated)
        errors = gate.check_measured(parsed, 101, None)
        self.assertTrue(any("did not complete" in e for e in errors), "\n".join(errors))

    def test_crate_the_report_never_reached_is_red(self) -> None:
        parsed = gate.parse_report(CLEAN_REPORT)
        scope = gate.CrateScope(checkable=["meerkat", "meerkat-core"])
        errors = gate.check_measured(parsed, 0, scope)
        self.assertEqual(len(errors), 1, "\n".join(errors))
        self.assertIn("meerkat-core", errors[0])
        self.assertNotIn("meerkat,", errors[0])

    def test_a_published_crate_with_no_manifest_is_red(self) -> None:
        parsed = gate.parse_report(CLEAN_REPORT)
        scope = gate.CrateScope(checkable=["meerkat"], missing_manifest=["ghost-crate"])
        errors = gate.check_measured(parsed, 0, scope)
        self.assertEqual(len(errors), 1, "\n".join(errors))
        self.assertIn("ghost-crate", errors[0])


class CrateScopeTests(unittest.TestCase):
    """Which published crates cargo-semver-checks can look at, from manifests.

    Observed from a real `--workspace` run: proc-macro crates produce NO output
    at all, not even a skip line. Requiring them would red-gate every release.
    """

    @classmethod
    def setUpClass(cls) -> None:
        cls.root = Path(__file__).resolve().parent.parent
        release = subprocess.run(
            [str(cls.root / "scripts" / "release-rust-crates.sh")],
            capture_output=True,
            text=True,
            check=True,
        )
        cls.release_crates = [line for line in release.stdout.split() if line]
        cls.scope = gate.classify_crates(cls.root, cls.release_crates)

    def test_every_release_crate_is_classified_exactly_once(self) -> None:
        classified = (
            self.scope.checkable
            + self.scope.proc_macro
            + self.scope.no_lib_target
            + self.scope.missing_manifest
        )
        self.assertEqual(sorted(classified), sorted(self.release_crates))
        self.assertEqual(self.scope.missing_manifest, [])

    def test_proc_macro_crates_are_out_of_scope(self) -> None:
        # These two are absent from a real --workspace report; the gate must
        # not demand a `Finished` line for them.
        self.assertIn("meerkat-machine-derive", self.scope.proc_macro)
        self.assertIn("meerkat-machine-dsl", self.scope.proc_macro)
        self.assertNotIn("meerkat-machine-derive", self.scope.checkable)

    def test_bin_only_crate_is_out_of_scope(self) -> None:
        self.assertIn("rkat", self.scope.no_lib_target)
        self.assertNotIn("rkat", self.scope.checkable)

    def test_library_crates_are_in_scope(self) -> None:
        for name in ("meerkat", "meerkat-core", "meerkat-sqlite", "meerkat-machine-dsl-core"):
            self.assertIn(name, self.scope.checkable)

    def test_real_full_workspace_report_covers_exactly_the_checkable_crates(self) -> None:
        # The strongest coverage evidence available: a complete real
        # `cargo semver-checks check-release --workspace` run. Every crate the
        # classifier calls checkable produced a `Finished` line, and every crate
        # it excludes produced nothing at all.
        parsed = gate.parse_report(WORKSPACE_REPORT)
        self.assertEqual(sorted(parsed.finished_crates), sorted(self.scope.checkable))
        self.assertEqual(gate.check_measured(parsed, 0, self.scope), [])
        for name in self.scope.proc_macro + self.scope.no_lib_target:
            self.assertNotIn(name, parsed.finished_crates)
            self.assertNotIn(name, parsed.checked_crates)


class CliAcceptanceTests(unittest.TestCase):
    """The three acceptance cases, end to end through the analyser CLI.

    The changelog under test is built from this repository's real 0.8.23
    release notes so the "correct" case is a real declared release rather than
    a fixture written to match the parser.
    """

    @classmethod
    def setUpClass(cls) -> None:
        start = REPO_CHANGELOG.index("## [0.8.23]")
        end = REPO_CHANGELOG.index("## [0.8.22]")
        cls.section = REPO_CHANGELOG[start:end]
        cls.head = "# Changelog\n\npolicy blurb\n\n"
        cls.tail = "## [0.8.22] - 2026-08-09\n\n- old\n"
        cls.tmp = tempfile.TemporaryDirectory()
        cls.report = str(FIXTURES / "report-meerkat-sqlite-0.8.22.txt")

    @classmethod
    def tearDownClass(cls) -> None:
        cls.tmp.cleanup()

    def run_gate(self, changelog_text: str) -> subprocess.CompletedProcess[str]:
        path = Path(self.tmp.name) / f"CHANGELOG-{self.id().rsplit('.', 1)[-1]}.md"
        path.write_text(changelog_text, encoding="utf-8")
        return subprocess.run(
            [
                sys.executable,
                str(SCRIPT),
                "--report",
                self.report,
                "--changelog",
                str(path),
                "--version",
                "0.8.23",
                "--tool-exit-code",
                "1",
            ],
            capture_output=True,
            text=True,
            check=False,
        )

    def test_c_correct_changelog_is_green(self) -> None:
        result = self.run_gate(self.head + "## [Unreleased]\n\n" + self.section + self.tail)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("7 public-API break(s) detected, all named", result.stdout)

    def test_a_omitted_break_is_red_and_names_the_missing_item(self) -> None:
        text = self.head + "## [Unreleased]\n\n" + self.section.replace("Copy", "Klone") + self.tail
        result = self.run_gate(text)
        self.assertEqual(result.returncode, 1, result.stdout)
        self.assertIn("MaintenanceBridgeReport no longer derives Copy", result.stderr)
        self.assertIn("missing: `Copy`", result.stderr)

    def test_b_unstamped_pending_section_is_red(self) -> None:
        body = self.section[self.section.index("\n") :]
        result = self.run_gate(self.head + "## [Unreleased]" + body + self.tail)
        self.assertEqual(result.returncode, 1, result.stdout)
        self.assertIn("would publish notes titled", result.stderr)

    def test_report_the_tool_could_not_produce_is_red(self) -> None:
        clean = Path(self.tmp.name) / "empty-report.txt"
        clean.write_text("error: failed to build rustdoc JSON\n", encoding="utf-8")
        changelog = Path(self.tmp.name) / "CHANGELOG-unmeasured.md"
        changelog.write_text(
            self.head + "## [Unreleased]\n\n" + self.section + self.tail, encoding="utf-8"
        )
        result = subprocess.run(
            [
                sys.executable,
                str(SCRIPT),
                "--report",
                str(clean),
                "--changelog",
                str(changelog),
                "--version",
                "0.8.23",
                "--tool-exit-code",
                "101",
            ],
            capture_output=True,
            text=True,
            check=False,
        )
        self.assertEqual(result.returncode, 1, result.stdout)
        self.assertIn("could not measure", result.stderr)


if __name__ == "__main__":
    unittest.main(verbosity=2)
