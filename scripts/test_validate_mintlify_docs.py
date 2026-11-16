#!/usr/bin/env python3
"""Regression tests for the local Mintlify documentation validator."""

from __future__ import annotations

import contextlib
import importlib.util
import inspect
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

# Connect keeps a copy of the checker for its own docs tree; both must hold
# the same slug rule.
CONNECT_CHECKER = ROOT / "connect" / "scripts" / "check-docs.py"
CONNECT_SPEC = importlib.util.spec_from_file_location("connect_check_docs", CONNECT_CHECKER)
if CONNECT_SPEC is None or CONNECT_SPEC.loader is None:
    raise RuntimeError(f"could not load {CONNECT_CHECKER}")
CONNECT = importlib.util.module_from_spec(CONNECT_SPEC)
CONNECT_SPEC.loader.exec_module(CONNECT)
CHECKERS = (("docs", VALIDATE), ("connect", CONNECT))

APOSTROPHE = chr(0x2019)
OPENING_QUOTE = chr(0x201C)
CLOSING_QUOTE = chr(0x201D)
LONG_DASH = chr(0x2014)
SECTION_SIGN = chr(0x00A7)

# Headings of the published site (docs.rkat.ai, read on 2026-11-13) with the
# id the site gives each: (page, heading as the source writes it, id).
PUBLISHED_HEADING_IDS = (
    # A comma, a colon, a semicolon and a question mark leave the id.
    ("guides/durable-runs", "One Writer, Fenced", "one-writer-fenced"),
    (
        "concepts/authorization",
        "Sources, peers, and knowledge services",
        "sources-peers-and-knowledge-services",
    ),
    ("api/mcp", "Pull streams: open, read, close", "pull-streams-open-read-close"),
    ("guides/auth", "The fast path: env vars", "the-fast-path-env-vars"),
    (
        "architecture/multi-host-mobs-plan",
        "11. Test matrix (lane-mapped; deterministic lanes are the CI ratchet)",
        "11-test-matrix-lane-mapped-deterministic-lanes-are-the-ci-ratchet",
    ),
    (
        "why-meerkat",
        "How Is This Different From Claude Code Or Codex?",
        "how-is-this-different-from-claude-code-or-codex",
    ),
    # A slash and the other escaped characters are in the id as themselves.
    ("meta/api/rpc", "`meta/identity/warm`", "meta/identity/warm"),
    ("api/rpc", "session/fork_at", "session/fork_at"),
    ("architecture/realm-config-inheritance-plan", "Phases & gates", "phases-&-gates"),
    (
        "meta/sdks/typescript",
        "Structural mob events + flow runs",
        "structural-mob-events-+-flow-runs",
    ),
    (
        "plans/durable-tool-execution-and-background-jobs-v1",
        "MobKit PR #301 composition",
        "mobkit-pr-#301-composition",
    ),
    (
        "architecture/multi-host-mobs-plan",
        f"14.5 Bind ceremony additions (folds into {SECTION_SIGN}7.2)",
        f"14-5-bind-ceremony-additions-folds-into-{SECTION_SIGN}7-2",
    ),
    (
        "meta/sdks/python",
        f"Comms {LONG_DASH} deliver work to agents",
        f"comms-{LONG_DASH}-deliver-work-to-agents",
    ),
    # A backslash escape renders its character; braces then leave the id.
    ("api/rest", "POST /sessions/\\{id\\}/messages", "post-/sessions/id/messages"),
    ("recall/architecture/domain-model", "Link\\<T\\>", "link<t>"),
    # A period, a tilde and an asterisk are hyphens, as before; the hyphens a
    # removed comma leaves side by side stay.
    (
        "concepts/configuration",
        "What About `~/.rkat/config.toml` And `.rkat/config.toml`?",
        "what-about-/-rkat/config-toml-and-rkat/config-toml",
    ),
    (
        "guides/upgrading",
        "Auth wire (`auth/login/*`, `auth/status/get`)",
        "auth-wire-auth/login/--auth/status/get",
    ),
    # A straight double quote in code leaves the id; in prose the renderer
    # curls it first and the curly quote stays.
    (
        "meta/guides/authentication",
        '`auth_config` with an external issuer (`provider: "oidc"`)',
        "auth_config-with-an-external-issuer-provider-oidc",
    ),
    (
        "meta/forge/troubleshooting",
        'Sign-in says "Too many login attempts"',
        f"sign-in-says-{OPENING_QUOTE}too-many-login-attempts{CLOSING_QUOTE}",
    ),
    # A straight apostrophe in prose is curled and stays in the id.
    ("concepts/configuration", "The host's own inputs", f"the-host{APOSTROPHE}s-own-inputs"),
    ("meta/quickstart", "What's next", f"what{APOSTROPHE}s-next"),
    (
        "guides/upgrading",
        "`rkat resume` and `meerkat_resume` refuse mob members' sessions",
        f"rkat-resume-and-meerkat_resume-refuse-mob-members{APOSTROPHE}-sessions",
    ),
    # Two hyphens in prose are one long dash.
    ("cli/commands", "run --resume", f"run-{LONG_DASH}resume"),
    ("recall/operations/scenarios", "S1.x -- Baseline CRUD", f"s1-x-{LONG_DASH}-baseline-crud"),
)


class PublishedHeadingIdTests(unittest.TestCase):
    """Both checkers must derive the id the published site gives a heading."""

    def test_checkers_derive_the_published_heading_ids(self) -> None:
        for name, checker in CHECKERS:
            for page, heading, published in PUBLISHED_HEADING_IDS:
                with self.subTest(checker=name, page=page, heading=heading):
                    self.assertEqual(checker.slugify(heading), published)

    def test_anchor_selects_the_id_a_browser_resolves(self) -> None:
        for name, checker in CHECKERS:
            with self.subTest(checker=name):
                # The fragment is percent-decoded and then compared with the ids.
                self.assertEqual(
                    checker.normalize_anchor("meta%2Fidentity%2Fwarm"), "meta/identity/warm"
                )
                self.assertEqual(
                    checker.normalize_anchor("meta/identity/warm"), "meta/identity/warm"
                )
                self.assertEqual(
                    checker.normalize_anchor("the-host%E2%80%99s-own-inputs"),
                    f"the-host{APOSTROPHE}s-own-inputs",
                )
                # An escaped comma decodes to a comma, which no id holds.
                self.assertEqual(
                    checker.normalize_anchor("one-writer%2C-fenced"), "one-writer,-fenced"
                )
                self.assertNotEqual(
                    checker.normalize_anchor("one-writer%2C-fenced"),
                    checker.slugify("One Writer, Fenced"),
                )

    def test_connect_copy_holds_the_same_slug_rule(self) -> None:
        for function in (
            "heading_characters",
            "typographic_text",
            "encoded_slug",
            "clean_heading_id",
            "slugify",
            "normalize_anchor",
            "heading_slugs",
        ):
            with self.subTest(function=function):
                self.assertEqual(
                    inspect.getsource(getattr(VALIDATE, function)),
                    inspect.getsource(getattr(CONNECT, function)),
                )


class MintlifyHeadingSlugTests(unittest.TestCase):
    def test_slash_is_escaped_in_the_slug_and_plain_in_the_id(self) -> None:
        self.assertEqual(VALIDATE.encoded_slug("capabilities/get"), "capabilities%2Fget")
        self.assertEqual(VALIDATE.slugify("capabilities/get"), "capabilities/get")

    def test_semicolon_is_escaped_in_the_slug_and_absent_from_the_id(self) -> None:
        self.assertEqual(
            VALIDATE.encoded_slug("Profiles are templates; members are declared"),
            "profiles-are-templates%3B-members-are-declared",
        )
        self.assertEqual(
            VALIDATE.slugify("Profiles are templates; members are declared"),
            "profiles-are-templates-members-are-declared",
        )

    def test_preserves_underscores_and_curls_apostrophes(self) -> None:
        self.assertEqual(VALIDATE.slugify("What's _new_?"), f"what{APOSTROPHE}s-_new_")

    def test_anchor_normalization_decodes_escapes_and_lower_cases(self) -> None:
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
    2026-08-21 while the heading's id was `capabilities/get` (in a link,
    `capabilities/get` or `capabilities%2Fget`); the validator
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

    def write_rpc_page(self, body: str) -> None:
        (self.docs / "api" / "rpc.mdx").write_text(
            f'---\ntitle: "RPC"\ndescription: "RPC"\nicon: "plug"\n---\n\n{body}',
            encoding="utf-8",
        )

    def run_validator(self) -> tuple[int, str]:
        stderr = io.StringIO()
        with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(stderr):
            code = VALIDATE.main()
        return code, stderr.getvalue()

    def test_comma_heading_accepts_the_published_id_and_rejects_the_escaped_comma(self) -> None:
        # The site gives `One writer, fenced` the id `one-writer-fenced`; a
        # fragment with `%2C` decodes to a comma and reaches no id.
        self.write_rpc_page("## One writer, fenced\n\nSee [above](#one-writer-fenced).\n")
        self.write_rest_page("/api/rpc#one-writer-fenced")
        code, errors = self.run_validator()
        self.assertEqual(code, 0, errors)

        self.write_rest_page("/api/rpc#one-writer%2C-fenced")
        code, errors = self.run_validator()
        self.assertEqual(code, 1)
        self.assertIn(
            "docs/api/rest.mdx links to missing anchor '/api/rpc#one-writer%2C-fenced'", errors
        )

        self.write_rest_page("/api/rpc#one-writer-fenced")
        self.write_rpc_page("## One writer, fenced\n\nSee [above](#one-writer%2C-fenced).\n")
        code, errors = self.run_validator()
        self.assertEqual(code, 1)
        self.assertIn("docs/api/rpc.mdx links to missing anchor '#one-writer%2C-fenced'", errors)

    def test_slash_heading_accepts_the_escaped_and_the_plain_slash(self) -> None:
        # The id is `capabilities/get`; a browser decodes `%2F` before it looks.
        for link in ("/api/rpc#capabilities%2Fget", "/api/rpc#capabilities/get"):
            with self.subTest(link=link):
                self.write_rest_page(link)
                code, errors = self.run_validator()
                self.assertEqual(code, 0, errors)

    def test_apostrophe_heading_accepts_the_curly_apostrophe_and_rejects_its_removal(self) -> None:
        self.write_rpc_page("## The host's own inputs\n")
        for link in (
            "/api/rpc#the-host%E2%80%99s-own-inputs",
            f"/api/rpc#the-host{APOSTROPHE}s-own-inputs",
        ):
            with self.subTest(link=link):
                self.write_rest_page(link)
                code, errors = self.run_validator()
                self.assertEqual(code, 0, errors)
        self.write_rest_page("/api/rpc#the-hosts-own-inputs")
        code, errors = self.run_validator()
        self.assertEqual(code, 1)
        self.assertIn(
            "docs/api/rest.mdx links to missing anchor '/api/rpc#the-hosts-own-inputs'", errors
        )

    def test_valid_page_and_mintlify_anchor_pass(self) -> None:
        self.write_rest_page("/api/rpc#capabilities%2Fget")
        code, errors = self.run_validator()
        self.assertEqual(code, 0, errors)

    def test_internal_docs_are_exempt_from_navigation_checks(self) -> None:
        self.write_rest_page("/api/rpc#capabilities%2Fget")
        internal = self.docs / "internal" / "notes"
        internal.mkdir(parents=True)
        (internal / "scratch.md").write_text("# Scratch\n\nNot a public page.\n", encoding="utf-8")
        (internal / "generated.json").write_text("{}", encoding="utf-8")
        code, errors = self.run_validator()
        self.assertEqual(code, 0, errors)

    def test_unlisted_public_page_still_fails(self) -> None:
        self.write_rest_page("/api/rpc#capabilities%2Fget")
        (self.docs / "api" / "orphan.mdx").write_text("# Orphan\n", encoding="utf-8")
        code, errors = self.run_validator()
        self.assertEqual(code, 1)
        self.assertIn("public docs file is not in docs.json navigation: docs/api/orphan.mdx", errors)

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

    def test_semicolon_heading_accepts_the_published_id_and_rejects_the_escaped_semicolon(self) -> None:
        # The site strips the semicolon from the id, so the fragment without it
        # is the one that resolves; `%3B` decodes to a semicolon no id holds.
        (self.docs / "api" / "rpc.mdx").write_text(
            '---\ntitle: "RPC"\ndescription: "RPC"\nicon: "plug"\n---\n\n'
            "## Profiles are templates; members are declared\n",
            encoding="utf-8",
        )
        self.write_rest_page("/api/rpc#profiles-are-templates-members-are-declared")
        code, errors = self.run_validator()
        self.assertEqual(code, 0, errors)
        self.write_rest_page("/api/rpc#profiles-are-templates%3B-members-are-declared")
        code, errors = self.run_validator()
        self.assertEqual(code, 1)
        self.assertIn("missing anchor", errors)


if __name__ == "__main__":
    unittest.main()
