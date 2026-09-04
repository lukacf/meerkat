#!/usr/bin/env python3
"""Regression tests for the local Mintlify documentation validator."""

from __future__ import annotations

import contextlib
import importlib.util
import io
import json
import tempfile
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
VALIDATOR = ROOT / "scripts" / "validate-mintlify-docs.py"

SPEC = importlib.util.spec_from_file_location("validate_mintlify_docs", VALIDATOR)
if SPEC is None or SPEC.loader is None:
    raise RuntimeError(f"could not load {VALIDATOR}")
VALIDATE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(VALIDATE)


class MintlifyHeadingSlugTests(unittest.TestCase):
    def test_percent_encodes_slash_like_mintlify(self) -> None:
        self.assertEqual(VALIDATE.slugify("capabilities/get"), "capabilities%2Fget")

    def test_preserves_underscores_and_removes_apostrophes(self) -> None:
        self.assertEqual(VALIDATE.slugify("What's _new_?"), "whats-_new_%3F")

    def test_anchor_normalization_keeps_percent_escapes_upper_case(self) -> None:
        self.assertEqual(
            VALIDATE.normalize_anchor("Capabilities%2fGet"),
            VALIDATE.slugify("capabilities/get"),
        )
        self.assertEqual(VALIDATE.normalize_anchor("#Repeat".lstrip("#")), "repeat")

    def test_duplicate_headings_start_at_suffix_two(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "duplicates.mdx"
            path.write_text("## Repeat\n## Repeat\n## Repeat\n", encoding="utf-8")
            self.assertEqual(
                VALIDATE.heading_slugs(path),
                {"repeat", "repeat-2", "repeat-3"},
            )



class MintlifyLinkValidationTests(unittest.TestCase):
    """The validator must fail on the link classes that once shipped live.

    docs/api/rest.mdx linked `/api/rpc#capabilitiesget` from 2026-05-10 to
    2026-08-21 while the heading rendered as `capabilities%2Fget`; the validator
    of the time slugified the slash away and stayed green. These tests drive
    `main()` against a temporary docs tree so a regression in either the
    missing-page or the missing-anchor check turns the validator red here.
    """

    def setUp(self) -> None:
        self._temp = tempfile.TemporaryDirectory()
        self.addCleanup(self._temp.cleanup)
        # resolve_link() resolves targets; ROOT is resolved in production too, and
        # macOS temp dirs live behind a /var -> /private/var symlink.
        root = Path(self._temp.name).resolve()
        self.docs = root / "docs"
        (self.docs / "api").mkdir(parents=True)
        self.original = (VALIDATE.ROOT, VALIDATE.DOCS, VALIDATE.DOCS_JSON)
        VALIDATE.ROOT = root
        VALIDATE.DOCS = self.docs
        VALIDATE.DOCS_JSON = self.docs / "docs.json"
        self.addCleanup(self.restore_validator_paths)
        (self.docs / "docs.json").write_text(
            json.dumps(
                {
                    "navigation": {
                        "tabs": [
                            {
                                "tab": "API",
                                "groups": [{"group": "API", "pages": ["api/rpc", "api/rest"]}],
                            }
                        ]
                    }
                }
            ),
            encoding="utf-8",
        )
        (self.docs / "api" / "rpc.mdx").write_text(
            '---\ntitle: "RPC"\ndescription: "RPC"\nicon: "plug"\n---\n\n## capabilities/get\n\nText.\n',
            encoding="utf-8",
        )

    def restore_validator_paths(self) -> None:
        VALIDATE.ROOT, VALIDATE.DOCS, VALIDATE.DOCS_JSON = self.original

    def write_rest_page(self, link: str) -> None:
        (self.docs / "api" / "rest.mdx").write_text(
            f'---\ntitle: "REST"\ndescription: "REST"\nicon: "globe"\n---\n\nSee [RPC]({link}).\n',
            encoding="utf-8",
        )

    def run_validator(self) -> tuple[int, str]:
        stderr = io.StringIO()
        with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(stderr):
            code = VALIDATE.main()
        return code, stderr.getvalue()

    def test_valid_page_and_mintlify_anchor_pass(self) -> None:
        self.write_rest_page("/api/rpc#capabilities%2Fget")
        code, errors = self.run_validator()
        self.assertEqual(code, 0, errors)

    def test_link_to_a_missing_page_fails(self) -> None:
        self.write_rest_page("/api/rpc-v2")
        code, errors = self.run_validator()
        self.assertEqual(code, 1)
        self.assertIn("docs/api/rest.mdx links to missing target '/api/rpc-v2'", errors)

    def test_link_to_a_missing_heading_anchor_fails(self) -> None:
        self.write_rest_page("/api/rpc#capabilitiesget")
        code, errors = self.run_validator()
        self.assertEqual(code, 1)
        self.assertIn("docs/api/rest.mdx links to missing anchor '/api/rpc#capabilitiesget'", errors)


if __name__ == "__main__":
    unittest.main()
