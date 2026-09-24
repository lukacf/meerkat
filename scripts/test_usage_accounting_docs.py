#!/usr/bin/env python3
"""Pin the usage-accounting worked example to the tests that prove it.

``docs/reference/usage-accounting.mdx`` documents one worked example over four
Anthropic-shaped provider calls in two runs of one session.
``crates/meerkat-core/src/agent/usage_accounting_tests.rs`` drives the real agent loop
and asserts what the event stream publishes for that script;
``crates/meerkat-core/src/types/tests.rs`` (module ``usage_aggregation_semantics``)
asserts the same arithmetic against ``CumulativeUsage``.

Editing either side alone would let the documented example drift away from
behavior while both files still look internally consistent. This gate parses the
page's worked-example table and compares it cell by cell against the expected
script, then requires every pinned number and every load-bearing assertion
symbol to be present on the Rust side. A single wrong cell fails here.
"""

from __future__ import annotations

import pathlib
import re
import unittest

ROOT = pathlib.Path(__file__).resolve().parent.parent
DOCS_PAGE = ROOT / "docs" / "reference" / "usage-accounting.mdx"
LOOP_TEST = ROOT / "crates" / "meerkat-core" / "src" / "agent" / "usage_accounting_tests.rs"
TYPES_TEST = ROOT / "crates" / "meerkat-core" / "src" / "types" / "tests.rs"
TYPES_TEST_MODULE = "usage_aggregation_semantics"

# The worked-example table, exactly as the page must render it:
# (run, call, uncached input, cache creation, cache read, presented, output,
#  emits a turn_completed row).
EXPECTED_TABLE_ROWS = (
    (1, 1, 1000, 4000, 0, 5000, 200, "no"),
    (1, 2, 300, 0, 4000, 4300, 150, "no"),
    (1, 3, 120, 0, 4300, 4420, 90, "yes"),
    (2, 4, 200, 0, 4500, 4700, 60, "yes"),
)

# Numbers the page derives from that table: the two observed run totals, the
# attributed subtotals, and the two documented wrong numbers.
EXPECTED_DERIVED_NUMBERS = (
    13720,  # run 1 cumulative input
    440,  # run 1 cumulative output
    14160,  # run 1 cumulative total
    18420,  # session cumulative input after run 2
    500,  # session cumulative output after run 2
    18920,  # session cumulative total after run 2
    9120,  # presented input the turn rows attribute
    150,  # output the turn rows attribute
    9270,  # total the turn rows attribute
    9650,  # session total no turn row attributes
    9300,  # run 1 input no turn row accounts for
    320,  # summing raw per-call input_tokens (wrong)
    33080,  # summing the two observed run totals (wrong)
    4510,  # comparable per-call total for the run-closing call
    210,  # raw per-call total for the same call
)

# Field paths the page tells consumers to read.
PINNED_PAGE_PATHS = (
    "accounting.presented_tokens",
    "run_completed.usage",
    "turn_completed.usage",
)

# The loop test must actually read the events and the normalized field, not
# merely mention them in prose.
PINNED_LOOP_ASSERTIONS = (
    "AgentEvent::TurnCompleted",
    "AgentEvent::RunCompleted",
    "presented_tokens()",
    "accounting().model",
)


def _types_test_module_source() -> str:
    text = TYPES_TEST.read_text(encoding="utf-8")
    marker = f"mod {TYPES_TEST_MODULE} {{"
    start = text.find(marker)
    if start == -1:
        raise AssertionError(f"{TYPES_TEST}: missing `mod {TYPES_TEST_MODULE}`")
    return text[start:]


def _rust_sources() -> str:
    return LOOP_TEST.read_text(encoding="utf-8") + _types_test_module_source()


def _number_forms(value: int) -> tuple[str, ...]:
    """Both the plain and the Rust underscore-separated spelling of `value`."""
    plain = str(value)
    if len(plain) <= 3:
        return (plain,)
    grouped = ""
    for index, digit in enumerate(reversed(plain)):
        if index and index % 3 == 0:
            grouped = "_" + grouped
        grouped = digit + grouped
    return (plain, grouped)


def _appears(text: str, value: int) -> bool:
    return any(
        re.search(rf"(?<![\d_]){form}(?![\d_])", text) for form in _number_forms(value)
    )


def _worked_example_table_rows() -> tuple[tuple[object, ...], ...]:
    """Parse the worked-example table into one tuple per data row.

    The table is identified by its header, so a rename of the header cells also
    fails this gate rather than silently matching some other table.
    """
    page = DOCS_PAGE.read_text(encoding="utf-8")
    rows: list[tuple[object, ...]] = []
    in_table = False
    for line in page.splitlines():
        stripped = line.strip()
        if not stripped.startswith("|"):
            if in_table:
                break
            continue
        cells = [cell.strip() for cell in stripped.strip("|").split("|")]
        cells = [cell.replace("`", "").replace("*", "") for cell in cells]
        if not in_table:
            if cells[:2] == ["Run", "Call"]:
                in_table = True
            continue
        if set("".join(cells)) <= set("-: "):
            continue
        parsed: list[object] = []
        for cell in cells:
            parsed.append(int(cell) if re.fullmatch(r"\d+", cell) else cell)
        rows.append(tuple(parsed))
    if not in_table:
        raise AssertionError(
            f"{DOCS_PAGE}: worked-example table (header `Run | Call | ...`) is gone"
        )
    return tuple(rows)


class UsageAccountingDocsPin(unittest.TestCase):
    def test_docs_page_exists_and_is_navigable(self) -> None:
        self.assertTrue(DOCS_PAGE.is_file(), f"missing {DOCS_PAGE}")
        docs_json = (ROOT / "docs" / "docs.json").read_text(encoding="utf-8")
        self.assertIn("reference/usage-accounting", docs_json)

    def test_worked_example_table_matches_the_pinned_script_cell_by_cell(self) -> None:
        self.assertEqual(
            _worked_example_table_rows(),
            EXPECTED_TABLE_ROWS,
            f"{DOCS_PAGE}: worked-example table no longer matches the pinned "
            "provider script asserted in "
            "crates/meerkat-core/src/agent/usage_accounting_tests.rs",
        )

    def test_table_cells_are_asserted_on_the_rust_side(self) -> None:
        rust = _rust_sources()
        for row in EXPECTED_TABLE_ROWS:
            for cell in row[2:]:
                if isinstance(cell, int):
                    self.assertTrue(
                        _appears(rust, cell),
                        f"the usage-accounting tests no longer assert {cell}",
                    )

    def test_derived_numbers_appear_on_both_sides(self) -> None:
        page = DOCS_PAGE.read_text(encoding="utf-8")
        rust = _rust_sources()
        for value in EXPECTED_DERIVED_NUMBERS:
            self.assertTrue(
                _appears(page, value),
                f"{DOCS_PAGE}: worked example lost the derived number {value}",
            )
            self.assertTrue(
                _appears(rust, value),
                f"the usage-accounting tests no longer assert {value}",
            )

    def test_pinned_field_paths_appear_on_both_sides(self) -> None:
        page = DOCS_PAGE.read_text(encoding="utf-8")
        loop_test = LOOP_TEST.read_text(encoding="utf-8")
        for path in PINNED_PAGE_PATHS:
            self.assertIn(path, page, f"{DOCS_PAGE}: lost guidance for {path}")
        for symbol in PINNED_LOOP_ASSERTIONS:
            self.assertIn(
                symbol,
                loop_test,
                f"{LOOP_TEST}: no longer exercises {symbol}, so the page's "
                "guidance is unpinned",
            )

    def test_docs_page_states_what_must_not_be_summed(self) -> None:
        page = DOCS_PAGE.read_text(encoding="utf-8")
        self.assertIn("## What not to sum", page)
        self.assertIn("double-count", page)

    def test_docs_page_states_the_cumulative_account_is_session_scoped(self) -> None:
        """The false invariant this gate exists to prevent coming back."""
        page = DOCS_PAGE.read_text(encoding="utf-8")
        self.assertIn("session-cumulative", page)
        self.assertIn("## Which calls emit a usage row", page)


if __name__ == "__main__":
    unittest.main()
