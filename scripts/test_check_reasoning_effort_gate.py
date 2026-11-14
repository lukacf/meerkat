#!/usr/bin/env python3
"""Tests of scripts/check_reasoning_effort_gate.py on synthetic trees."""

from __future__ import annotations

import sys
import tempfile
import unittest
from pathlib import Path

SCRIPTS = Path(__file__).resolve().parent
sys.path.insert(0, str(SCRIPTS))

import check_reasoning_effort_gate as gate  # noqa: E402

ENTRY = gate.ENTRY_MARKER + " A provider-neutral\n  `reasoning_effort` level can be set on every request surface.\n"


def changelog(unreleased: str = "", released: dict[str, str] | None = None) -> str:
    text = "# Changelog\n\n## [Unreleased]\n\n### Added\n\n" + unreleased + "\n"
    for version, body in (released or {}).items():
        text += f"## [{version}] - 2026-01-01\n\n### Added\n\n{body}\n"
    return text


def tree(root: Path, *, log: str, workspace: str, python: str | None, typescript: str | None) -> Path:
    (root / "sdks/python/meerkat").mkdir(parents=True)
    (root / "sdks/typescript/src").mkdir(parents=True)
    (root / "CHANGELOG.md").write_text(log, encoding="utf-8")
    (root / "Cargo.toml").write_text(
        f'[workspace]\nmembers = []\n\n[workspace.package]\nversion = "{workspace}"\nedition = "2024"\n',
        encoding="utf-8",
    )
    python_line = "" if python is None else f"_REASONING_EFFORT_MIN_CONTRACT = ({python})\n"
    (root / "sdks/python/meerkat/client.py").write_text(
        "_GRANT_GATED_TOOLS_MIN_CONTRACT = (0, 9, 9)\n" + python_line, encoding="utf-8"
    )
    typescript_line = (
        "" if typescript is None
        else f"const REASONING_EFFORT_MIN_CONTRACT: readonly number[] = [{typescript}];\n"
    )
    (root / "sdks/typescript/src/client.ts").write_text(
        'const MEERKAT_REPO = "example/repo";\n' + typescript_line, encoding="utf-8"
    )
    return root


class GateCheck(unittest.TestCase):
    def setUp(self) -> None:
        self._dir = tempfile.TemporaryDirectory()
        self.root = Path(self._dir.name)
        self.addCleanup(self._dir.cleanup)

    def problems(self, **kwargs) -> list[str]:
        return gate.check(tree(self.root, **kwargs))

    def test_an_unreleased_entry_expects_the_next_patch_release(self) -> None:
        self.assertEqual(
            self.problems(log=changelog(ENTRY), workspace="1.4.2", python="1, 4, 3", typescript="1, 4, 3"),
            [],
        )

    def test_an_unreleased_entry_with_another_constant_fails(self) -> None:
        # A release cut that left the entry under Unreleased: the workspace
        # moved to 1.4.3 and the constants still name it.
        problems = self.problems(
            log=changelog(ENTRY), workspace="1.4.3", python="1, 4, 3", typescript="1, 4, 3"
        )
        self.assertEqual(len(problems), 2, problems)
        self.assertIn("`_REASONING_EFFORT_MIN_CONTRACT` in sdks/python/meerkat/client.py", problems[0])
        self.assertIn("names 1.4.3 and must name 1.4.4", problems[0])
        self.assertIn("Change the constant in sdks/python/meerkat/client.py to 1.4.4", problems[0])
        self.assertIn("under [Unreleased]", problems[0])
        self.assertIn("`REASONING_EFFORT_MIN_CONTRACT` in sdks/typescript/src/client.ts", problems[1])

    def test_one_stale_constant_is_named_alone(self) -> None:
        problems = self.problems(
            log=changelog(ENTRY), workspace="1.4.2", python="1, 4, 3", typescript="1, 4, 2"
        )
        self.assertEqual(len(problems), 1, problems)
        self.assertIn("sdks/typescript/src/client.ts", problems[0])
        self.assertIn("names 1.4.2 and must name 1.4.3", problems[0])

    def test_a_released_entry_expects_its_own_section(self) -> None:
        log = changelog("- Something later.\n", {"1.4.3": ENTRY, "1.4.2": "- Something earlier.\n"})
        # The workspace version no longer matters once the entry is released.
        self.assertEqual(
            self.problems(log=log, workspace="1.5.0", python="1, 4, 3", typescript="1, 4, 3"), []
        )

    def test_a_released_entry_with_another_constant_fails(self) -> None:
        log = changelog("", {"1.4.3": ENTRY})
        problems = self.problems(log=log, workspace="1.4.3", python="1, 4, 4", typescript="1, 4, 4")
        self.assertEqual(len(problems), 2, problems)
        self.assertIn("names 1.4.4 and must name 1.4.3", problems[0])
        self.assertIn("under [1.4.3]", problems[0])

    def test_a_missing_entry_or_constant_fails_closed(self) -> None:
        problems = self.problems(
            log=changelog("- Something else.\n"), workspace="1.4.2", python="1, 4, 3", typescript="1, 4, 3"
        )
        self.assertEqual(len(problems), 1, problems)
        self.assertIn("is there 0 times", problems[0])

    def test_a_missing_constant_fails_closed(self) -> None:
        other = Path(tempfile.mkdtemp(dir=self.root))
        problems = gate.check(
            tree(other, log=changelog(ENTRY), workspace="1.4.2", python=None, typescript="1, 4, 3")
        )
        self.assertEqual(len(problems), 1, problems)
        self.assertIn(
            "sdks/python/meerkat/client.py: the gate constant `_REASONING_EFFORT_MIN_CONTRACT` is there 0 times",
            problems[0],
        )

    def test_the_command_exits_with_the_verdict(self) -> None:
        tree(self.root, log=changelog(ENTRY), workspace="1.4.2", python="1, 4, 3", typescript="1, 4, 3")
        self.assertEqual(gate.main(["--root", str(self.root)]), 0)
        (self.root / "Cargo.toml").write_text(
            '[workspace.package]\nversion = "1.4.3"\n', encoding="utf-8"
        )
        self.assertEqual(gate.main(["--root", str(self.root)]), 1)


if __name__ == "__main__":
    unittest.main()
