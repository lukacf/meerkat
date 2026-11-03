#!/usr/bin/env python3
"""Focused compatibility tests for scripts/memory-evals profile loading."""

import importlib.machinery
import importlib.util
import unittest
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parent.parent
SCRIPT = REPO_ROOT / "scripts" / "memory-evals"
LOADER = importlib.machinery.SourceFileLoader("memory_evals", str(SCRIPT))
SPEC = importlib.util.spec_from_loader(LOADER.name, LOADER)
MEMORY_EVALS = importlib.util.module_from_spec(SPEC)
LOADER.exec_module(MEMORY_EVALS)


class ProfileTomlCompatibilityTests(unittest.TestCase):
    def test_fallback_decodes_current_profiles_with_expected_types(self):
        expected_stages = {"selector", "distiller", "steward", "hygienist"}
        actual_stages = set()

        for path in sorted((REPO_ROOT / "tests" / "memory-evals" / "profiles").glob("*.toml")):
            profile = MEMORY_EVALS.parse_profile_toml(
                path.read_text(), force_fallback=True
            )
            actual_stages.add(profile["stage"])
            self.assertIsInstance(profile["version"], str)
            self.assertIsInstance(profile["model"], str)
            self.assertIsInstance(profile["prompt_bundle"], str)
            self.assertIsInstance(profile["params"], dict)
            self.assertIsInstance(profile["params"]["temperature"], float)

        self.assertEqual(actual_stages, expected_stages)

    @unittest.skipIf(MEMORY_EVALS._tomllib is None, "stdlib tomllib unavailable")
    def test_fallback_matches_stdlib_for_current_profiles(self):
        for path in sorted((REPO_ROOT / "tests" / "memory-evals" / "profiles").glob("*.toml")):
            text = path.read_text()
            self.assertEqual(
                MEMORY_EVALS.parse_profile_toml(text, force_fallback=True),
                MEMORY_EVALS.parse_profile_toml(text),
                path.name,
            )

    def test_fallback_preserves_scalar_semantics_and_inline_comments(self):
        profile = MEMORY_EVALS.parse_profile_toml(
            """
stage = "selector # primary"
version = '1'
model = "provider=model"
prompt_bundle = "prompts/selector-v0.md" # resolved from the eval root
escaped = "line\\nnext"

[params]
temperature = 0.0
max_output_tokens = 2_048
shuffle_manifest = true
penalty = -1.25e+2
""",
            force_fallback=True,
        )

        self.assertEqual(profile["stage"], "selector # primary")
        self.assertEqual(profile["model"], "provider=model")
        self.assertEqual(profile["escaped"], "line\nnext")
        self.assertEqual(profile["params"]["max_output_tokens"], 2048)
        self.assertIs(profile["params"]["shuffle_manifest"], True)
        self.assertEqual(profile["params"]["penalty"], -125.0)

    def test_fallback_rejects_duplicate_keys(self):
        with self.assertRaisesRegex(
            MEMORY_EVALS.ProfileTomlDecodeError, "duplicate key `stage`"
        ):
            MEMORY_EVALS.parse_profile_toml(
                'stage = "selector"\nstage = "distiller"\n', force_fallback=True
            )

    def test_fallback_rejects_richer_toml_instead_of_misparsing_it(self):
        with self.assertRaisesRegex(
            MEMORY_EVALS.ProfileTomlDecodeError, "unsupported value"
        ):
            MEMORY_EVALS.parse_profile_toml(
                'stage = "selector"\nmodels = ["a", "b"]\n', force_fallback=True
            )


if __name__ == "__main__":
    unittest.main()
