#!/usr/bin/env python3

from __future__ import annotations

import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import textwrap
import unittest


REPO_ROOT = Path(
    os.environ.get("DOGMA_GATE_REPO_ROOT", Path(__file__).resolve().parents[2])
).resolve()
SCRIPT = Path(
    os.environ.get("DOGMA_GATE_SCRIPT", REPO_ROOT / "scripts" / "dogma_cleanup_gate.py")
).resolve()
FIXTURE_DIR = Path(
    os.environ.get(
        "DOGMA_GATE_FIXTURE_DIR",
        REPO_ROOT / "scripts" / "fixtures" / "dogma_cleanup_gate",
    )
).resolve()
WORKFLOW_EXEC_ROOT = Path(os.environ.get("DOGMA_GATE_WORKFLOW_EXEC_ROOT", REPO_ROOT)).resolve()


def run_command(args: list[str], cwd: Path) -> subprocess.CompletedProcess[str]:
    return subprocess.run(args, cwd=cwd, text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)


def require_success(args: list[str], cwd: Path) -> str:
    result = run_command(args, cwd)
    if result.returncode != 0:
        raise AssertionError(
            f"{args} failed with {result.returncode}\nstdout:\n{result.stdout}\nstderr:\n{result.stderr}"
        )
    return result.stdout.strip()


def run_gate(args: list[str]) -> subprocess.CompletedProcess[str]:
    env = os.environ.copy()
    env.pop("DOGMA_CLEANUP_BASE", None)
    return subprocess.run(
        [sys.executable, str(SCRIPT), *args],
        cwd=REPO_ROOT,
        env=env,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )


class DogmaCleanupGateTest(unittest.TestCase):
    maxDiff = None

    def test_packet_fixture_passes(self) -> None:
        result = run_gate(
            [
                "--packet",
                str(FIXTURE_DIR / "passing_packet.md"),
                "--head-sha",
                "0000000000000000000000000000000000000000",
                "--packet-only",
            ]
        )
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("Dogma cleanup gate passed", result.stdout)

    def test_wrapper_only_packet_fails(self) -> None:
        result = run_gate(
            [
                "--packet",
                str(FIXTURE_DIR / "wrapper_only_packet.md"),
                "--head-sha",
                "0000000000000000000000000000000000000000",
                "--packet-only",
            ]
        )
        self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("wrapped is not an amputation classification", result.stderr)

    def test_preferred_path_only_packet_fails(self) -> None:
        result = run_gate(
            [
                "--packet",
                str(FIXTURE_DIR / "preferred_path_only_packet.md"),
                "--head-sha",
                "0000000000000000000000000000000000000000",
                "--packet-only",
            ]
        )
        self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("preferred-path-only cleanup leaves the old path callable", result.stderr)

    def test_callable_wrapper_packet_fails(self) -> None:
        result = run_gate(
            [
                "--packet",
                str(FIXTURE_DIR / "callable_wrapper_packet.md"),
                "--head-sha",
                "0000000000000000000000000000000000000000",
                "--packet-only",
            ]
        )
        self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("proof admits the old path remains callable", result.stderr)

    def test_compatibility_supported_packet_fails(self) -> None:
        result = run_gate(
            [
                "--packet",
                str(FIXTURE_DIR / "compatibility_supported_packet.md"),
                "--head-sha",
                "0000000000000000000000000000000000000000",
                "--packet-only",
            ]
        )
        self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("compatibility-retained proof leaves the old path callable", result.stderr)

    def test_migration_usable_packet_fails(self) -> None:
        result = run_gate(
            [
                "--packet",
                str(FIXTURE_DIR / "migration_usable_packet.md"),
                "--head-sha",
                "0000000000000000000000000000000000000000",
                "--packet-only",
            ]
        )
        self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("proof admits old callers can still use the old path", result.stderr)

    def test_caller_legacy_endpoint_packet_fails(self) -> None:
        result = run_gate(
            [
                "--packet",
                str(FIXTURE_DIR / "caller_legacy_endpoint_packet.md"),
                "--head-sha",
                "0000000000000000000000000000000000000000",
                "--packet-only",
            ]
        )
        self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("proof admits old callers can still use the old path", result.stderr)

    def test_modal_intent_retained_packet_fails(self) -> None:
        result = run_gate(
            [
                "--packet",
                str(FIXTURE_DIR / "modal_intent_retained_packet.md"),
                "--head-sha",
                "0000000000000000000000000000000000000000",
                "--packet-only",
            ]
        )
        self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("proof admits old callers can still use the old path", result.stderr)

    def test_retained_endpoint_synonym_packets_fail(self) -> None:
        for proof in [
            "Rejected calls prove the boundary while the legacy endpoint is preserved for migration.",
            "Rejected calls prove the boundary while the previous route stays enabled during rollout.",
            "Rejected calls prove the boundary while the legacy endpoint was preserved for migration.",
            "Rejected calls prove the boundary while the previous route was enabled during rollout.",
            "Rejected calls prove the boundary while the endpoint was preserved for migration.",
            "Rejected calls prove the boundary while the endpoint was enabled during rollout.",
            "Rejected calls prove the boundary while that route was preserved.",
            "Rejected calls prove the boundary while that route was enabled.",
            "Rejected calls prove the boundary while retained the endpoint for migration.",
            "Rejected calls prove the boundary while the endpoint was maintained for migration.",
            "Rejected calls prove the boundary while the endpoint was allowed during rollout.",
            "Rejected calls prove the boundary while maintained the endpoint for migration.",
            "Rejected calls prove the boundary while legacy access remained allowed during migration.",
            "Rejected calls prove the boundary while the legacy endpoint continues responding during migration.",
            "Rejected calls prove the boundary while the legacy endpoint continues to serve old clients.",
            "Rejected calls prove the boundary while old clients are served by the legacy endpoint during migration.",
            "Rejected calls prove the boundary while the old route keeps accepting requests for compatibility.",
            "Rejected calls prove the boundary while the legacy API still accepts requests during rollout.",
            "Rejected calls prove the boundary while old clients can still invoke the legacy endpoint during migration.",
            "Rejected calls prove the boundary while callers keep hitting the old route.",
            "Rejected calls prove the boundary while callers keep hitting the old action.",
            "Rejected calls prove the boundary while users may still submit to the deprecated API.",
            "Rejected calls prove the boundary while the legacy export remains importable.",
            "Rejected calls prove the boundary while the legacy endpoint remains fully operational.",
            "Rejected calls prove the boundary while the legacy endpoint stays open.",
            "Rejected calls prove the boundary while the legacy endpoint remains in service.",
            "Rejected calls prove the boundary while older integrations can continue through it.",
            "Rejected calls prove the boundary while previous exports keep functioning for old integrations.",
            "Rejected calls prove the boundary while existing callers remain on the legacy surface until LUC-999.",
            "Rejected calls prove the boundary while the former endpoint remains reachable during migration.",
            "Rejected calls prove the boundary while deprecated access remains available during rollout.",
            "Rejected calls prove the boundary while the legacy service remains available during migration.",
            "Rejected calls prove the boundary while retained the service for migration.",
            "Rejected calls prove the boundary while retaining the service for migration.",
            "Rejected calls prove the boundary while the PR retains the service for migration.",
            "Rejected calls prove the boundary while keeping the legacy service available during migration.",
            "Rejected calls prove the boundary while the legacy service is retained for migration.",
            "Rejected calls prove the boundary while the service is retained for migration.",
            "Rejected calls prove the boundary while legacy access is retained during migration.",
            "Rejected calls prove the boundary while the route is kept during migration.",
            "Rejected calls prove the boundary while route is kept.",
            "Rejected calls prove the boundary while access is kept.",
            "Rejected calls prove the boundary while helper is left.",
            "Rejected calls prove the boundary while path continues working.",
            "Rejected calls prove the boundary while route continues to respond.",
            "Rejected calls prove the boundary while the old route continues to be callable.",
            "Rejected calls prove the boundary while endpoint continues responding for compatibility.",
            "Rejected calls prove the boundary while caller is kept until LUC-999.",
            "Rejected calls prove the boundary while clients keep using the legacy endpoint during migration.",
            "Rejected calls prove the boundary while legacy SDKs continue using it during migration.",
            "Rejected calls prove the boundary while older clients will keep using the legacy endpoint.",
            "Rejected calls prove the boundary while callers kept calling the old route.",
            "Rejected calls prove the boundary while keeping the client during migration.",
            "Rejected calls prove the boundary while maintains the integration during migration.",
            "Rejected calls prove the boundary while keeps the access during migration.",
            "Rejected calls prove the boundary while the previous endpoint will stay accessible during migration.",
            "Rejected calls prove the boundary while the legacy endpoint will remain reachable.",
            "Rejected calls prove the boundary while the legacy endpoint remains exposed for semver stability.",
            "Rejected calls prove the boundary while the old route stays published for ABI stability.",
            "Rejected calls prove the boundary while the legacy API is grandfathered for existing integrations.",
        ]:
            with self.subTest(proof=proof):
                result = run_gate(
                    [
                        "--packet",
                        str(self.packet_with_proof(proof)),
                        "--head-sha",
                        "0000000000000000000000000000000000000000",
                        "--packet-only",
                    ]
                )
                self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
                self.assertIn("proof admits old callers can still use the old path", result.stderr)

    def test_modal_intent_retained_variants_fail(self) -> None:
        for proof in [
            "Rejected calls prove the boundary while old clients should continue using the legacy endpoint.",
            "Rejected calls prove the boundary while old clients might still invoke the legacy endpoint.",
            "Rejected calls prove the boundary while callers need to continue hitting the old route.",
            "Rejected calls prove the boundary while the legacy route is intended to stay reachable.",
            "Rejected calls prove the boundary while old clients must continue using the legacy endpoint.",
            "Rejected calls prove the boundary while old clients are expected to keep using the legacy endpoint.",
            "Rejected calls prove the boundary while old clients ought to continue invoking the legacy endpoint.",
            "Rejected calls prove the boundary while the old helper should remain invocable.",
            "Rejected calls prove the boundary while the legacy endpoint might still be callable.",
            "Rejected calls prove the boundary while SDKs are expected to keep using the legacy API.",
        ]:
            with self.subTest(proof=proof):
                result = run_gate(
                    [
                        "--packet",
                        str(self.packet_with_proof(proof)),
                        "--head-sha",
                        "0000000000000000000000000000000000000000",
                        "--packet-only",
                    ]
                )
                self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
                self.assertIn("proof admits old callers can still use the old path", result.stderr)

    def test_test_only_boundary_proof_fails(self) -> None:
        result = run_gate(
            [
                "--packet",
                str(
                    self.packet_with_proof(
                        "A test-only harness rejects calls; the old route is not callable in that fixture."
                    )
                ),
                "--head-sha",
                "0000000000000000000000000000000000000000",
                "--packet-only",
            ]
        )
        self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("test-only proof does not prove production boundary uncallability", result.stderr)

    def test_unit_test_fixture_rejected_calls_need_production_boundary_evidence(self) -> None:
        result = run_gate(
            [
                "--packet",
                str(
                    self.packet_with_proof(
                        "A unit test fixture rejects calls and says the old route is not callable."
                    )
                ),
                "--head-sha",
                "0000000000000000000000000000000000000000",
                "--packet-only",
            ]
        )
        self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("cites only non-production fixture", result.stderr)

    def test_bare_make_boundary_packet_fails(self) -> None:
        result = run_gate(
            [
                "--packet",
                str(FIXTURE_DIR / "bare_make_boundary_packet.md"),
                "--head-sha",
                "0000000000000000000000000000000000000000",
                "--packet-only",
            ]
        )
        self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("cites only non-production fixture", result.stderr)

    def test_bare_make_or_cargo_does_not_make_fixture_proof_production_boundary(self) -> None:
        for proof in [
            "A unit test fixture rejects calls and says the old route is not callable; make test passed.",
            "A unit test fixture rejects calls and says the old route is not callable; cargo test passed.",
        ]:
            with self.subTest(proof=proof):
                result = run_gate(
                    [
                        "--packet",
                        str(self.packet_with_proof(proof)),
                        "--head-sha",
                        "0000000000000000000000000000000000000000",
                        "--packet-only",
                    ]
                )
                self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
                self.assertIn("cites only non-production fixture", result.stderr)

    def test_fixture_proof_can_pass_with_production_boundary_evidence(self) -> None:
        result = run_gate(
            [
                "--packet",
                str(
                    self.packet_with_proof(
                        "A unit test fixture rejects calls, and the production router removed "
                        "the old route from the public route registry."
                    )
                ),
                "--head-sha",
                "0000000000000000000000000000000000000000",
                "--packet-only",
            ]
        )
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_unit_test_fixture_with_unrelated_production_noun_fails(self) -> None:
        result = run_gate(
            [
                "--packet",
                str(
                    self.packet_with_proof(
                        "A unit test fixture rejects calls; production docs mention the boundary."
                    )
                ),
                "--head-sha",
                "0000000000000000000000000000000000000000",
                "--packet-only",
            ]
        )
        self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("cites only non-production fixture", result.stderr)

    def test_alias_facade_and_shadow_route_retention_packets_fail(self) -> None:
        proofs = [
            "Rejected calls prove the boundary; the legacy route is an alias during rollout.",
            "Rejected calls prove the boundary; legacy consumers are migrated through the old facade until LUC-999.",
            "Rejected calls prove the boundary; the previous shadow route handles compatibility traffic.",
            "Rejected calls prove the boundary; the old route remains as an alias.",
            "Rejected calls prove the boundary; the old endpoint remains behind a facade.",
            "Rejected calls prove the boundary; the previous endpoint remains as a shadow route.",
            "Rejected calls prove the boundary; the previous endpoint remains on a shadow surface.",
        ]
        for proof in proofs:
            with self.subTest(proof=proof):
                result = run_gate(
                    [
                        "--packet",
                        str(self.packet_with_proof(proof)),
                        "--head-sha",
                        "0000000000000000000000000000000000000000",
                        "--packet-only",
                    ]
                )
                self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
                self.assertIn("old path callable", result.stderr)

    def test_compatibility_traffic_on_previous_endpoint_fails(self) -> None:
        result = run_gate(
            [
                "--packet",
                str(
                    self.packet_with_proof(
                        "Rejected calls prove the boundary. Compatibility traffic is handled by the previous endpoint."
                    )
                ),
                "--head-sha",
                "0000000000000000000000000000000000000000",
                "--packet-only",
            ]
        )
        self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("compatibility-retained proof leaves the old path callable", result.stderr)

    def test_retention_language_outside_parsed_cells_fails(self) -> None:
        packet = self.packet_with_proof(
            "Rejected calls prove the boundary and the old route is not callable."
        )
        text = packet.read_text(encoding="utf-8")
        text = text.replace(
            "## Complexity Delta",
            "Note: the legacy endpoint remains callable for compatibility until LUC-999.\n\n"
            "## Complexity Delta",
        )
        packet.write_text(text, encoding="utf-8")

        result = run_gate(
            [
                "--packet",
                str(packet),
                "--head-sha",
                "0000000000000000000000000000000000000000",
                "--packet-only",
            ]
        )
        self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("proof admits the old path remains callable", result.stderr)

    def test_retention_language_in_extra_table_cell_fails(self) -> None:
        packet = self.packet_with_proof(
            "Rejected calls prove the boundary and the old route is not callable."
        )
        text = packet.read_text(encoding="utf-8")
        text = text.replace(
            "| `legacy_endpoint` | made uncallable at boundary | `legacy_endpoint` | "
            "Rejected calls prove the boundary and the old route is not callable. | none |",
            "| `legacy_endpoint` | made uncallable at boundary | `legacy_endpoint` | "
            "Rejected calls prove the boundary and the old route is not callable. | none | "
            "the legacy endpoint remains callable for compatibility until LUC-999 |",
        )
        packet.write_text(text, encoding="utf-8")

        result = run_gate(
            [
                "--packet",
                str(packet),
                "--head-sha",
                "0000000000000000000000000000000000000000",
                "--packet-only",
            ]
        )
        self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("proof admits the old path remains callable", result.stderr)

    def test_retained_follow_up_language_fails(self) -> None:
        proof = "Rejected calls prove the boundary and the old route is not callable."
        for follow_up in [
            "LUC-999 keeps the endpoint preserved for migration.",
            "LUC-999 leaves legacy access allowed during migration.",
            "LUC-999 keeps the legacy endpoint responding during migration.",
            "LUC-999 keeps the legacy service available during migration.",
            "LUC-999 retained the service for migration.",
            "LUC-999 keeps the service for migration.",
            "LUC-999 keeping the legacy service available during migration.",
        ]:
            with self.subTest(follow_up=follow_up):
                result = run_gate(
                    [
                        "--packet",
                        str(self.packet_with_proof(proof, follow_up=follow_up)),
                        "--head-sha",
                        "0000000000000000000000000000000000000000",
                        "--packet-only",
                    ]
                )
                self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
                self.assertIn("proof admits old callers can still use the old path", result.stderr)

    def test_current_head_sha_must_match_current_head_line(self) -> None:
        expected_head = "1111111111111111111111111111111111111111"
        stale_head = "2222222222222222222222222222222222222222"
        proof = (
            "Rejected calls prove the boundary and the old route is not callable. "
            f"Validation command used {expected_head}."
        )
        result = run_gate(
            [
                "--packet",
                str(self.packet_with_proof(proof, head_sha=stale_head)),
                "--head-sha",
                expected_head,
                "--packet-only",
            ]
        )
        self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("Current head SHA must be", result.stderr)

        prefix_result = run_gate(
            [
                "--packet",
                str(self.packet_with_proof(proof, head_sha=expected_head[:12])),
                "--head-sha",
                expected_head,
                "--packet-only",
            ]
        )
        self.assertNotEqual(prefix_result.returncode, 0, prefix_result.stdout + prefix_result.stderr)
        self.assertIn("Current head SHA", prefix_result.stderr)

        masked_result = run_gate(
            [
                "--packet",
                str(
                    self.packet_with_proof(
                        proof,
                        head_sha=stale_head,
                        preamble=f"## Validation\n\nCurrent head SHA: {expected_head}\n\n",
                    )
                ),
                "--head-sha",
                expected_head,
                "--packet-only",
            ]
        )
        self.assertNotEqual(masked_result.returncode, 0, masked_result.stdout + masked_result.stderr)
        self.assertIn("Current head SHA must be", masked_result.stderr)

    def test_fenced_headings_cannot_spoof_packet_sections(self) -> None:
        head_sha = "0000000000000000000000000000000000000000"
        packet = tempfile.NamedTemporaryFile("w", delete=False, encoding="utf-8")
        self.addCleanup(lambda: Path(packet.name).unlink(missing_ok=True))
        with packet:
            packet.write(
                textwrap.dedent(
                    f"""\
                    ## Review Readiness Packet

                    Current head SHA: {head_sha}
                    Command output: fake local gate output.

                    ```markdown
                    ## Old Path Amputation Proof

                    | Old path | Classification | Search literal | Mechanical proof | Follow-up |
                    | --- | --- | --- | --- | --- |
                    | `legacy_endpoint` | made uncallable at boundary | `legacy_endpoint` | Rejected calls prove the boundary and the old route is not callable. | none |
                    ```

                    ## Old Path Amputation Proof

                    | Old path | Classification | Search literal | Mechanical proof | Follow-up |
                    | --- | --- | --- | --- | --- |
                    | `legacy_endpoint` | made uncallable at boundary | `legacy_endpoint` | Rejected calls prove the boundary while old clients can still invoke the legacy endpoint during migration. | none |

                    ## Complexity Delta

                    - Docs/scripts/tests: 1 file.
                    - Non-test implementation: 0 files.
                    - Generated/schema churn: 0 files.

                    ## Sample Passing Old Path Amputation Proof

                    | Old path | Classification | Search literal | Mechanical proof | Follow-up |
                    | --- | --- | --- | --- | --- |
                    | `old/session-route` | deleted | `old/session-route` | No production references remain. | none |

                    ## Sample Failing Old Path Amputation Proof

                    | Old path | Classification | Search literal | Mechanical proof | Follow-up |
                    | --- | --- | --- | --- | --- |
                    | `old/session-route` | wrapped | `old/session-route` | A preferred path exists beside the old route. | none |
                    """
                )
            )
        result = run_gate(
            [
                "--packet",
                packet.name,
                "--head-sha",
                head_sha,
                "--packet-only",
            ]
        )
        self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("proof admits old callers can still use the old path", result.stderr)

    def test_long_fenced_headings_cannot_spoof_packet_sections(self) -> None:
        head_sha = "0000000000000000000000000000000000000000"
        packet = tempfile.NamedTemporaryFile("w", delete=False, encoding="utf-8")
        self.addCleanup(lambda: Path(packet.name).unlink(missing_ok=True))
        with packet:
            packet.write(
                textwrap.dedent(
                    f"""\
                    ## Review Readiness Packet

                    Current head SHA: {head_sha}
                    Command output: fake local gate output.

                    ````markdown
                    ## Old Path Amputation Proof

                    | Old path | Classification | Search literal | Mechanical proof | Follow-up |
                    | --- | --- | --- | --- | --- |
                    | `legacy_endpoint` | made uncallable at boundary | `legacy_endpoint` | Rejected calls prove the boundary and the old route is not callable. | none |
                    ```

                    ## Complexity Delta

                    - Docs/scripts/tests: 1 file.
                    - Non-test implementation: 0 files.
                    - Generated/schema churn: 0 files.

                    ## Sample Passing Old Path Amputation Proof

                    | Old path | Classification | Search literal | Mechanical proof | Follow-up |
                    | --- | --- | --- | --- | --- |
                    | `old/session-route` | deleted | `old/session-route` | No production references remain. | none |

                    ## Sample Failing Old Path Amputation Proof

                    | Old path | Classification | Search literal | Mechanical proof | Follow-up |
                    | --- | --- | --- | --- | --- |
                    | `old/session-route` | wrapped | `old/session-route` | A preferred path exists beside the old route. | none |
                    ````
                    """
                )
            )
        result = run_gate(
            [
                "--packet",
                packet.name,
                "--head-sha",
                head_sha,
                "--packet-only",
            ]
        )
        self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("missing required heading: Old Path Amputation Proof", result.stderr)

    def test_retained_packet_fails_front_door_gate(self) -> None:
        result = run_gate(
            [
                "--packet",
                str(FIXTURE_DIR / "retained_packet.md"),
                "--head-sha",
                "0000000000000000000000000000000000000000",
                "--packet-only",
            ]
        )
        self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("retained old path `wasm_contract_string_mirror` is non-clean residual", result.stderr)

    def test_boundary_keyword_only_packet_fails(self) -> None:
        result = run_gate(
            [
                "--packet",
                str(FIXTURE_DIR / "boundary_keyword_only_packet.md"),
                "--head-sha",
                "0000000000000000000000000000000000000000",
                "--packet-only",
            ]
        )
        self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("must cite mechanical boundary proof", result.stderr)

    def test_default_base_falls_back_to_local_main(self) -> None:
        repo = self.create_repo()
        readme = repo / "README.md"
        readme.write_text("base\n", encoding="utf-8")
        require_success(["git", "add", "README.md"], repo)
        require_success(["git", "commit", "-q", "-m", "base"], repo)

        result = run_gate(
            [
                "--repo",
                str(repo),
                "--packet",
                str(FIXTURE_DIR / "passing_packet.md"),
                "--head-sha",
                "0000000000000000000000000000000000000000",
            ]
        )
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("Dogma cleanup gate passed", result.stdout)

    def test_deleted_old_path_requires_related_search_literal(self) -> None:
        repo = self.create_repo()
        source_path = repo / "src" / "lib.rs"
        source_path.parent.mkdir(parents=True, exist_ok=True)
        source_path.write_text("pub fn legacy_endpoint() {}\n", encoding="utf-8")
        require_success(["git", "add", "src/lib.rs"], repo)
        require_success(["git", "commit", "-q", "-m", "base"], repo)
        require_success(["git", "checkout", "-q", "-b", "pr"], repo)
        readme = repo / "README.md"
        readme.write_text("metadata refresh\n", encoding="utf-8")
        require_success(["git", "add", "README.md"], repo)
        require_success(["git", "commit", "-q", "-m", "metadata"], repo)

        packet = tempfile.NamedTemporaryFile("w", delete=False, encoding="utf-8")
        self.addCleanup(lambda: Path(packet.name).unlink(missing_ok=True))
        with packet:
            packet.write(
                textwrap.dedent(
                    """\
                    ## Review Readiness Packet

                    Current head SHA: 0000000000000000000000000000000000000000
                    Command output: fake local gate output.

                    ## Old Path Amputation Proof

                    | Old path | Classification | Search literal | Mechanical proof | Follow-up |
                    | --- | --- | --- | --- | --- |
                    | `legacy_endpoint` | deleted | `definitely_not_in_repo` | No production references remain. | none |

                    ## Complexity Delta

                    - Docs/scripts/tests: 1 file.
                    - Non-test implementation: 0 files.
                    - Generated/schema churn: 0 files.

                    ## Sample Passing Old Path Amputation Proof

                    | Old path | Classification | Search literal | Mechanical proof | Follow-up |
                    | --- | --- | --- | --- | --- |
                    | `old/session-route` | deleted | `old/session-route` | No production references remain. | none |

                    ## Sample Failing Old Path Amputation Proof

                    | Old path | Classification | Search literal | Mechanical proof | Follow-up |
                    | --- | --- | --- | --- | --- |
                    | `old/session-route` | wrapped | `old/session-route` | A preferred path exists beside the old route. | none |
                    """
                )
            )
        result = run_gate(
            [
                "--repo",
                str(repo),
                "--base",
                "main",
                "--packet",
                packet.name,
                "--head-sha",
                "0000000000000000000000000000000000000000",
            ]
        )
        self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("search literal `definitely_not_in_repo` must name or overlap", result.stderr)

    def test_uncallable_old_path_requires_related_search_literal(self) -> None:
        result = run_gate(
            [
                "--packet",
                str(
                    self.packet_with_proof(
                        "Rejected calls prove the production router boundary and the old route is not callable.",
                        search_literal="definitely_not_in_repo",
                    )
                ),
                "--head-sha",
                "0000000000000000000000000000000000000000",
                "--packet-only",
            ]
        )
        self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("uncallable old path `legacy_endpoint` search literal", result.stderr)
        self.assertIn("must name or overlap the old path", result.stderr)

    def test_uncallable_old_path_fails_when_search_literal_still_in_production(self) -> None:
        repo = self.create_repo()
        (repo / "Cargo.toml").write_text(
            textwrap.dedent(
                """\
                [workspace]
                members = ["app"]
                """
            ),
            encoding="utf-8",
        )
        source_path = repo / "app/src/lib.rs"
        source_path.parent.mkdir(parents=True, exist_ok=True)
        source_path.write_text(
            "pub fn legacy_endpoint() -> bool { true }\n"
            'pub fn canonical_owner() -> &\'static str { "new" }\n',
            encoding="utf-8",
        )
        require_success(["git", "add", "Cargo.toml", "app/src/lib.rs"], repo)
        require_success(["git", "commit", "-q", "-m", "base"], repo)
        require_success(["git", "checkout", "-q", "-b", "pr"], repo)

        result = run_gate(
            [
                "--repo",
                str(repo),
                "--base",
                "main",
                "--packet",
                str(
                    self.packet_with_proof(
                        "Rejected calls prove the production router boundary and the old route is not callable."
                    )
                ),
                "--head-sha",
                "0000000000000000000000000000000000000000",
            ]
        )
        self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn(
            "uncallable old path `legacy_endpoint` still has production references",
            result.stderr,
        )
        self.assertIn("app/src/lib.rs", result.stderr)

    def test_deleted_old_path_fails_when_normalized_literal_still_in_production(self) -> None:
        repo = self.create_repo()
        (repo / "Cargo.toml").write_text(
            textwrap.dedent(
                """\
                [workspace]
                members = ["app"]
                """
            ),
            encoding="utf-8",
        )
        source_path = repo / "app/src/lib.rs"
        source_path.parent.mkdir(parents=True, exist_ok=True)
        source_path.write_text("pub fn legacy_endpoint() -> bool { true }\n", encoding="utf-8")
        require_success(["git", "add", "Cargo.toml", "app/src/lib.rs"], repo)
        require_success(["git", "commit", "-q", "-m", "base"], repo)
        require_success(["git", "checkout", "-q", "-b", "pr"], repo)
        (repo / "README.md").write_text("metadata refresh\n", encoding="utf-8")
        require_success(["git", "add", "README.md"], repo)
        require_success(["git", "commit", "-q", "-m", "metadata"], repo)

        packet = tempfile.NamedTemporaryFile("w", delete=False, encoding="utf-8")
        self.addCleanup(lambda: Path(packet.name).unlink(missing_ok=True))
        with packet:
            packet.write(
                textwrap.dedent(
                    """\
                    ## Review Readiness Packet

                    Current head SHA: 0000000000000000000000000000000000000000
                    Command output: fake local gate output.

                    ## Old Path Amputation Proof

                    | Old path | Classification | Search literal | Mechanical proof | Follow-up |
                    | --- | --- | --- | --- | --- |
                    | `legacy_endpoint` | deleted | `legacy endpoint` | No production references remain. | none |

                    ## Complexity Delta

                    - Docs/scripts/tests: 1 file.
                    - Non-test implementation: 0 files.
                    - Generated/schema churn: 0 files.

                    ## Sample Passing Old Path Amputation Proof

                    | Old path | Classification | Search literal | Mechanical proof | Follow-up |
                    | --- | --- | --- | --- | --- |
                    | `old/session-route` | deleted | `old/session-route` | No production references remain. | none |

                    ## Sample Failing Old Path Amputation Proof

                    | Old path | Classification | Search literal | Mechanical proof | Follow-up |
                    | --- | --- | --- | --- | --- |
                    | `old/session-route` | wrapped | `old/session-route` | A preferred path exists beside the old route. | none |
                    """
                )
            )

        result = run_gate(
            [
                "--repo",
                str(repo),
                "--base",
                "main",
                "--packet",
                packet.name,
                "--head-sha",
                "0000000000000000000000000000000000000000",
            ]
        )
        self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn(
            "deleted old path `legacy_endpoint` still has production references",
            result.stderr,
        )
        self.assertIn("app/src/lib.rs", result.stderr)

    def test_deleted_schema_file_counts_as_generated_churn(self) -> None:
        repo = self.create_repo()
        schema_path = repo / "schemas" / "runtime.schema.json"
        schema_path.parent.mkdir(parents=True, exist_ok=True)
        schema_path.write_text('{"type":"object"}\n', encoding="utf-8")
        require_success(["git", "add", "schemas/runtime.schema.json"], repo)
        require_success(["git", "commit", "-q", "-m", "base"], repo)
        require_success(["git", "checkout", "-q", "-b", "pr"], repo)
        schema_path.unlink()
        require_success(["git", "add", "schemas/runtime.schema.json"], repo)
        require_success(["git", "commit", "-q", "-m", "delete schema"], repo)

        result = run_gate(
            [
                "--repo",
                str(repo),
                "--base",
                "main",
                "--packet",
                str(FIXTURE_DIR / "passing_packet.md"),
                "--head-sha",
                "0000000000000000000000000000000000000000",
            ]
        )
        self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("changed generated/schema paths exist", result.stderr)
        self.assertIn("schemas/runtime.schema.json", result.stderr)

    def test_deleted_repo_native_generated_rust_counts_as_generated_churn(self) -> None:
        repo = self.create_repo()
        generated_path = repo / "meerkat-runtime" / "src" / "generated" / "protocol.rs"
        generated_path.parent.mkdir(parents=True, exist_ok=True)
        generated_path.write_text("pub const GENERATED: &str = \"old\";\n", encoding="utf-8")
        require_success(["git", "add", "meerkat-runtime/src/generated/protocol.rs"], repo)
        require_success(["git", "commit", "-q", "-m", "base"], repo)
        require_success(["git", "checkout", "-q", "-b", "pr"], repo)
        generated_path.unlink()
        require_success(["git", "add", "meerkat-runtime/src/generated/protocol.rs"], repo)
        require_success(["git", "commit", "-q", "-m", "delete generated rust"], repo)

        result = run_gate(
            [
                "--repo",
                str(repo),
                "--base",
                "main",
                "--packet",
                str(FIXTURE_DIR / "passing_packet.md"),
                "--head-sha",
                "0000000000000000000000000000000000000000",
            ]
        )
        self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("changed generated/schema paths exist", result.stderr)
        self.assertIn("meerkat-runtime/src/generated/protocol.rs", result.stderr)

    def test_unsignaled_runtime_core_source_cleanup_requires_packet(self) -> None:
        result = self.run_unsignaled_source_cleanup(
            "meerkat-core/src/turn_execution_authority.rs",
            'pub fn legacy_runtime_metadata_cache() -> &\'static str { "old" }\n',
        )
        self.assert_stable_signal_required(result, "meerkat-core/src/turn_execution_authority.rs")

    def test_unsignaled_provider_model_owner_cleanup_requires_packet_without_legacy_keywords(self) -> None:
        result = self.run_unsignaled_source_cleanup(
            "meerkat-core/src/provider_model.rs",
            'pub fn provider_model_owner_source() -> &\'static str { "old" }\n',
        )
        self.assert_stable_signal_required(
            result,
            "meerkat-core/src/provider_model.rs",
            "production workspace crate source deletion",
        )

    def test_unsignaled_comms_source_cleanup_requires_packet_without_legacy_keywords(self) -> None:
        result = self.run_unsignaled_source_cleanup(
            "meerkat-comms/src/peer_model.rs",
            'pub struct PeerModelOwnerSource;\n',
        )
        self.assert_stable_signal_required(
            result,
            "meerkat-comms/src/peer_model.rs",
            "production workspace crate source deletion",
        )

    def test_unsignaled_cli_source_cleanup_requires_packet_from_workspace_manifest(self) -> None:
        result = self.run_unsignaled_source_cleanup(
            "meerkat-cli/src/auth.rs",
            'pub fn cli_owner_source() -> bool { true }\n',
        )
        self.assert_stable_signal_required(
            result,
            "meerkat-cli/src/auth.rs",
            "production workspace crate source deletion",
        )

    def test_removed_workspace_member_source_deletion_requires_packet(self) -> None:
        repo = self.create_repo()
        (repo / "Cargo.toml").write_text(
            textwrap.dedent(
                """\
                [workspace]
                members = ["old-crate"]
                """
            ),
            encoding="utf-8",
        )
        source_path = repo / "old-crate/src/lib.rs"
        source_path.parent.mkdir(parents=True, exist_ok=True)
        source_path.write_text(
            "pub fn removed_workspace_owner_source() -> bool { true }\n"
            'pub fn canonical_owner() -> &\'static str { "new" }\n',
            encoding="utf-8",
        )
        require_success(["git", "add", "Cargo.toml", "old-crate/src/lib.rs"], repo)
        require_success(["git", "commit", "-q", "-m", "base"], repo)
        require_success(["git", "checkout", "-q", "-b", "pr"], repo)

        (repo / "Cargo.toml").write_text(
            textwrap.dedent(
                """\
                [workspace]
                members = []
                """
            ),
            encoding="utf-8",
        )
        source_path.unlink()
        require_success(["git", "add", "Cargo.toml", "old-crate/src/lib.rs"], repo)
        require_success(["git", "commit", "-q", "-m", "remove workspace member source"], repo)
        head_sha = require_success(["git", "rev-parse", "HEAD"], repo)

        result = run_gate(
            [
                "--repo",
                str(repo),
                "--base",
                "main",
                "--github-event",
                str(self.github_event_path(head_sha)),
            ]
        )
        self.assert_stable_signal_required(
            result,
            "old-crate/src/lib.rs",
            "production workspace crate source deletion",
        )

    def test_unsignaled_rpc_surface_source_cleanup_requires_packet(self) -> None:
        result = self.run_unsignaled_source_cleanup(
            "meerkat-rpc/src/handlers/session.rs",
            'pub fn surface_request_lifecycle_fallback_cache() -> &\'static str { "old" }\n',
        )
        self.assert_stable_signal_required(result, "meerkat-rpc/src/handlers/session.rs")

    def test_unsignaled_enum_variant_authority_cleanup_requires_packet(self) -> None:
        result = self.run_unsignaled_source_cleanup(
            "meerkat-runtime/src/meerkat_machine/visibility.rs",
            "    ShellOwned,\n",
        )
        self.assert_stable_signal_required(
            result,
            "meerkat-runtime/src/meerkat_machine/visibility.rs",
            "production workspace crate source deletion",
        )

    def test_unsignaled_impl_only_authority_cleanup_requires_packet(self) -> None:
        result = self.run_unsignaled_source_cleanup(
            "meerkat-runtime/src/meerkat_machine/owner.rs",
            "impl BoundaryOwner for RuntimeOwner {}\n",
        )
        self.assert_stable_signal_required(
            result,
            "meerkat-runtime/src/meerkat_machine/owner.rs",
            "production workspace crate source deletion",
        )

    def test_unsignaled_source_cleanup_requires_packet_in_session_tools_and_mcp_server(self) -> None:
        cases = [
            ("meerkat-session/src/persistent.rs", 'pub fn session_owner_source() -> bool { true }\n'),
            ("meerkat-tools/src/registry.rs", 'pub struct ToolRegistryOwnerSource;\n'),
            ("meerkat-mcp-server/src/router.rs", 'pub fn mcp_router_owner_source() -> bool { true }\n'),
        ]
        for rel_path, deleted_line in cases:
            with self.subTest(rel_path=rel_path):
                result = self.run_unsignaled_source_cleanup(rel_path, deleted_line)
                self.assert_stable_signal_required(
                    result,
                    rel_path,
                    "production workspace crate source deletion",
                )

    def test_unsignaled_gate_owning_paths_require_packet(self) -> None:
        cases = [
            (
                "Makefile",
                "dogma-cleanup-ci-gate:\n\tpython3 scripts/dogma_cleanup_gate.py\n",
                "dogma-cleanup-ci-gate:\n\t@true\n",
            ),
            (".github/workflows/ci.yml", "name: CI\n", "name: build\n"),
            ("scripts/dogma_cleanup_gate.py", "print('enforce')\n", "print('skip')\n"),
            (
                "scripts/tests/dogma_cleanup_gate_test.py",
                "assert True\n",
                "assert True\n# changed\n",
            ),
            ("scripts/fixtures/dogma_cleanup_gate/passing_packet.md", "old\n", "new\n"),
            (
                ".github/workflows/dogma-cleanup-immutable-gate.yml",
                "name: immutable\n",
                "name: noop\n",
            ),
        ]
        for rel_path, base_text, pr_text in cases:
            with self.subTest(rel_path=rel_path):
                result = self.run_unsignaled_path_change(rel_path, base_text, pr_text)
                self.assert_stable_signal_required(result, rel_path, "dogma gate-owning path")

    def test_make_ci_gate_fails_closed_without_event_or_packet(self) -> None:
        override = tempfile.NamedTemporaryFile("w", delete=False, encoding="utf-8")
        self.addCleanup(lambda: Path(override.name).unlink(missing_ok=True))
        with override:
            override.write(
                ".PHONY: dogma-cleanup-gate-fixtures\n"
                "dogma-cleanup-gate-fixtures:\n"
                "\t@:\n"
            )
        env = os.environ.copy()
        env.pop("DOGMA_PACKET", None)
        env.pop("GITHUB_EVENT_PATH", None)
        result = subprocess.run(
            [
                "make",
                "--no-print-directory",
                "-f",
                "Makefile",
                "-f",
                override.name,
                "dogma-cleanup-ci-gate",
            ],
            cwd=REPO_ROOT,
            env=env,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
        self.assertEqual(result.returncode, 2, result.stdout + result.stderr)
        self.assertIn(
            "DOGMA_PACKET or GITHUB_EVENT_PATH is required for dogma cleanup CI enforcement",
            result.stderr,
        )
        self.assertNotIn("Dogma cleanup gate skipped", result.stdout + result.stderr)

    def test_local_broad_lane_gate_skips_without_event_or_packet(self) -> None:
        override = tempfile.NamedTemporaryFile("w", delete=False, encoding="utf-8")
        self.addCleanup(lambda: Path(override.name).unlink(missing_ok=True))
        with override:
            override.write(
                ".PHONY: dogma-cleanup-gate-fixtures\n"
                "dogma-cleanup-gate-fixtures:\n"
                "\t@:\n"
            )
        env = os.environ.copy()
        env.pop("DOGMA_PACKET", None)
        env.pop("GITHUB_EVENT_PATH", None)
        result = subprocess.run(
            [
                "make",
                "--no-print-directory",
                "-f",
                "Makefile",
                "-f",
                override.name,
                "dogma-cleanup-local-gate",
            ],
            cwd=REPO_ROOT,
            env=env,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("Dogma cleanup gate skipped for local broad lane", result.stdout)

    def test_broad_make_lanes_use_local_dogma_gate(self) -> None:
        makefile = (REPO_ROOT / "Makefile").read_text(encoding="utf-8")

        self.assertIn("agent-gate: dogma-cleanup-local-gate", makefile)
        self.assertIn("ci: dogma-cleanup-local-gate", makefile)
        self.assertIn("ci-smoke: dogma-cleanup-local-gate", makefile)
        self.assertIn("dogma-cleanup-ci-gate: dogma-cleanup-gate-fixtures", makefile)

    def test_github_gate_invokes_immutable_checker_without_pr_head_make(self) -> None:
        workflow = (REPO_ROOT / ".github" / "workflows" / "ci.yml").read_text(encoding="utf-8")

        self.assertIn("Checkout immutable gate source", workflow)
        self.assertIn("dogma-gate-source/scripts/dogma_cleanup_gate.py", workflow)
        self.assertIn('python3 "$gate_script" --repo "$pr_repo"', workflow)
        self.assertIn("Validate PR-head dogma gate fixtures", workflow)
        self.assertIn('pr_test_script="$pr_repo/scripts/tests/dogma_cleanup_gate_test.py"', workflow)
        self.assertIn('base_test_script="$GITHUB_WORKSPACE/dogma-gate-source/scripts/tests/dogma_cleanup_gate_test.py"', workflow)
        self.assertIn('DOGMA_GATE_SCRIPT="$pr_gate_script"', workflow)
        self.assertIn('[[ ! -f "$pr_test_script" ]]', workflow)
        self.assertIn("PR-head dogma gate fixture suite is missing", workflow)
        self.assertIn('python3 "$base_test_script" -v', workflow)
        self.assertIn('python3 "$pr_test_script" -v', workflow)
        self.assertNotIn("make dogma-cleanup-ci-gate", workflow)

    def test_pull_request_target_gate_uses_base_checker_and_base_owned_oracle(self) -> None:
        workflow = (
            REPO_ROOT / ".github" / "workflows" / "dogma-cleanup-immutable-gate.yml"
        ).read_text(encoding="utf-8")

        self.assertIn("pull_request_target:", workflow)
        self.assertIn("Dogma cleanup immutable review gate", workflow)
        self.assertIn("name: CI gate", workflow)
        self.assertIn("github.event.pull_request.head.repo.full_name", workflow)
        self.assertIn("github.event.pull_request.head.sha", workflow)
        self.assertIn("github.event.pull_request.base.sha", workflow)
        self.assertIn("persist-credentials: false", workflow)
        self.assertIn("Drop checkout credentials", workflow)
        self.assertIn(
            "git -C \"$repo\" config --local --unset-all http.https://github.com/.extraheader",
            workflow,
        )
        self.assertIn("includeIf\\..*\\.path", workflow)
        self.assertIn("dogma-gate-source/scripts/dogma_cleanup_gate.py", workflow)
        self.assertIn("refs/remotes/dogma-base/dogma-cleanup-base", workflow)
        self.assertIn('python3 "$gate_script" --repo "$pr_repo"', workflow)
        self.assertIn("Detect dogma gate-owned changes", workflow)
        self.assertIn("id: dogma_gate_changes", workflow)
        self.assertIn(".github/workflows/dogma-cleanup-immutable-gate.yml", workflow)
        self.assertIn("Validate immutable dogma gate behavior", workflow)
        self.assertIn("steps.dogma_gate_changes.outputs.changed == 'true'", workflow)
        self.assertIn(
            'base_test_script="$GITHUB_WORKSPACE/dogma-gate-source/scripts/tests/dogma_cleanup_gate_test.py"',
            workflow,
        )
        self.assertIn(
            'base_fixture_dir="$GITHUB_WORKSPACE/dogma-gate-source/scripts/fixtures/dogma_cleanup_gate"',
            workflow,
        )
        self.assertIn('pr_gate_script="$pr_repo/scripts/dogma_cleanup_gate.py"', workflow)
        self.assertIn('pr_test_script="$pr_repo/scripts/tests/dogma_cleanup_gate_test.py"', workflow)
        self.assertIn('pr_fixture_dir="$pr_repo/scripts/fixtures/dogma_cleanup_gate"', workflow)
        self.assertIn('DOGMA_GATE_REPO_ROOT="$pr_repo"', workflow)
        self.assertIn(
            'DOGMA_GATE_WORKFLOW_EXEC_ROOT="$GITHUB_WORKSPACE/dogma-gate-source"',
            workflow,
        )
        self.assertIn('DOGMA_GATE_SCRIPT="${pr_gate_script}"', workflow)
        self.assertIn('DOGMA_GATE_FIXTURE_DIR="$base_fixture_dir"', workflow)
        self.assertIn('DOGMA_GATE_FIXTURE_DIR="$pr_fixture_dir"', workflow)
        self.assertIn('python3 "$base_test_script" -v', workflow)
        self.assertNotIn("make dogma-cleanup-ci-gate", workflow)
        self.assertNotIn("python3 \"$pr_test_script\"", workflow)

    def test_immutable_fixture_step_uses_base_oracle_against_pr_gate_and_fixtures(self) -> None:
        body = self.workflow_step_body(
            "Validate immutable dogma gate behavior",
            ".github/workflows/dogma-cleanup-immutable-gate.yml",
        )
        temp_dir = tempfile.TemporaryDirectory()
        self.addCleanup(temp_dir.cleanup)
        workspace = Path(temp_dir.name)

        pr_gate = workspace / "pr" / "scripts" / "dogma_cleanup_gate.py"
        pr_gate.parent.mkdir(parents=True, exist_ok=True)
        pr_gate.write_text("raise SystemExit(0)\n", encoding="utf-8")
        pr_test = workspace / "pr" / "scripts" / "tests" / "dogma_cleanup_gate_test.py"
        pr_test.parent.mkdir(parents=True, exist_ok=True)
        pr_test.write_text(
            "from pathlib import Path\n"
            "Path(__file__).with_name('pr-ran.txt').write_text('bad', encoding='utf-8')\n"
            "raise SystemExit(0)\n",
            encoding="utf-8",
        )
        pr_fixture_dir = workspace / "pr" / "scripts" / "fixtures" / "dogma_cleanup_gate"
        pr_fixture_dir.mkdir(parents=True, exist_ok=True)

        base_test = (
            workspace
            / "dogma-gate-source"
            / "scripts"
            / "tests"
            / "dogma_cleanup_gate_test.py"
        )
        base_test.parent.mkdir(parents=True, exist_ok=True)
        base_fixture_dir = (
            workspace / "dogma-gate-source" / "scripts" / "fixtures" / "dogma_cleanup_gate"
        )
        base_fixture_dir.mkdir(parents=True, exist_ok=True)
        base_test.write_text(
            "from pathlib import Path\n"
            "import os\n"
            "marker = Path(__file__).with_name('base-ran.txt')\n"
            "with marker.open('a', encoding='utf-8') as handle:\n"
            "    handle.write(os.environ['DOGMA_GATE_REPO_ROOT'] + '\\n')\n"
            "    handle.write(os.environ['DOGMA_GATE_WORKFLOW_EXEC_ROOT'] + '\\n')\n"
            "    handle.write(os.environ['DOGMA_GATE_SCRIPT'] + '\\n')\n"
            "    handle.write(os.environ['DOGMA_GATE_FIXTURE_DIR'] + '\\n---\\n')\n",
            encoding="utf-8",
        )

        result = self.run_workflow_step_body(body, workspace)

        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertFalse((pr_test.parent / "pr-ran.txt").exists())
        marker = (base_test.parent / "base-ran.txt").read_text(encoding="utf-8")
        self.assertIn(str(workspace / "pr"), marker)
        self.assertIn(str(workspace / "dogma-gate-source"), marker)
        self.assertEqual(marker.count(str(pr_gate)), 2, marker)
        self.assertIn(str(base_fixture_dir), marker)
        self.assertIn(str(pr_fixture_dir), marker)

    def test_immutable_fixture_step_rejects_pr_head_noop_gate_with_base_fixture(self) -> None:
        body = self.workflow_step_body(
            "Validate immutable dogma gate behavior",
            ".github/workflows/dogma-cleanup-immutable-gate.yml",
        )
        temp_dir = tempfile.TemporaryDirectory()
        self.addCleanup(temp_dir.cleanup)
        workspace = Path(temp_dir.name)

        pr_gate = workspace / "pr" / "scripts" / "dogma_cleanup_gate.py"
        pr_gate.parent.mkdir(parents=True, exist_ok=True)
        pr_gate.write_text("raise SystemExit(0)\n", encoding="utf-8")
        pr_test = workspace / "pr" / "scripts" / "tests" / "dogma_cleanup_gate_test.py"
        pr_test.parent.mkdir(parents=True, exist_ok=True)
        pr_test.write_text("raise SystemExit(0)\n", encoding="utf-8")
        (workspace / "pr" / "scripts" / "fixtures" / "dogma_cleanup_gate").mkdir(
            parents=True, exist_ok=True
        )

        base_test = (
            workspace
            / "dogma-gate-source"
            / "scripts"
            / "tests"
            / "dogma_cleanup_gate_test.py"
        )
        base_test.parent.mkdir(parents=True, exist_ok=True)
        base_fixture_dir = (
            workspace / "dogma-gate-source" / "scripts" / "fixtures" / "dogma_cleanup_gate"
        )
        base_fixture_dir.mkdir(parents=True, exist_ok=True)
        (base_fixture_dir / "wrapper_only_packet.md").write_text(
            (FIXTURE_DIR / "wrapper_only_packet.md").read_text(encoding="utf-8"),
            encoding="utf-8",
        )
        base_test.write_text(
            "import os\n"
            "from pathlib import Path\n"
            "import subprocess\n"
            "import sys\n"
            "fixture = Path(os.environ['DOGMA_GATE_FIXTURE_DIR']) / 'wrapper_only_packet.md'\n"
            "result = subprocess.run(\n"
            "    [\n"
            "        sys.executable,\n"
            "        os.environ['DOGMA_GATE_SCRIPT'],\n"
            "        '--packet',\n"
            "        str(fixture),\n"
            "        '--head-sha',\n"
            "        '0000000000000000000000000000000000000000',\n"
            "        '--packet-only',\n"
            "    ],\n"
            "    text=True,\n"
            "    stdout=subprocess.PIPE,\n"
            "    stderr=subprocess.PIPE,\n"
            ")\n"
            "if result.returncode == 0:\n"
            "    print('base-owned dogma oracle rejected PR-head no-op gate for wrapper-only sample', file=sys.stderr)\n"
            "    raise SystemExit(1)\n",
            encoding="utf-8",
        )

        result = self.run_workflow_step_body(body, workspace)

        self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn(
            "base-owned dogma oracle rejected PR-head no-op gate for wrapper-only sample",
            result.stdout + result.stderr,
        )

    def test_pr_head_fixture_step_fails_closed_when_suite_is_missing_or_renamed(self) -> None:
        body = self.workflow_step_body("Validate PR-head dogma gate fixtures")
        temp_dir = tempfile.TemporaryDirectory()
        self.addCleanup(temp_dir.cleanup)
        workspace = Path(temp_dir.name)
        renamed = workspace / "pr" / "scripts" / "tests" / "dogma_cleanup_gate_tests.py"
        renamed.parent.mkdir(parents=True, exist_ok=True)
        renamed.write_text("print('renamed suite should not count')\n", encoding="utf-8")

        result = self.run_workflow_step_body(body, workspace)

        self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("PR-head dogma gate fixture suite is missing", result.stdout + result.stderr)

    def test_pr_head_fixture_step_bootstraps_pr_suite_when_base_oracle_is_absent(self) -> None:
        body = self.workflow_step_body("Validate PR-head dogma gate fixtures")
        temp_dir = tempfile.TemporaryDirectory()
        self.addCleanup(temp_dir.cleanup)
        workspace = Path(temp_dir.name)
        script = workspace / "pr" / "scripts" / "tests" / "dogma_cleanup_gate_test.py"
        script.parent.mkdir(parents=True, exist_ok=True)
        gate = workspace / "pr" / "scripts" / "dogma_cleanup_gate.py"
        gate.write_text("raise SystemExit(0)\n", encoding="utf-8")
        script.write_text(
            "from pathlib import Path\n"
            "import sys\n"
            "Path(__file__).with_name('ran.txt').write_text(' '.join(sys.argv[1:]), encoding='utf-8')\n",
            encoding="utf-8",
        )

        result = self.run_workflow_step_body(body, workspace)

        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual((script.parent / "ran.txt").read_text(encoding="utf-8"), "-v")

    def test_pr_head_fixture_step_uses_base_oracle_against_pr_gate_script(self) -> None:
        body = self.workflow_step_body("Validate PR-head dogma gate fixtures")
        temp_dir = tempfile.TemporaryDirectory()
        self.addCleanup(temp_dir.cleanup)
        workspace = Path(temp_dir.name)
        pr_test = workspace / "pr" / "scripts" / "tests" / "dogma_cleanup_gate_test.py"
        pr_test.parent.mkdir(parents=True, exist_ok=True)
        pr_test.write_text(
            "from pathlib import Path\n"
            "Path(__file__).with_name('pr-ran.txt').write_text('bad', encoding='utf-8')\n"
            "raise SystemExit(0)\n",
            encoding="utf-8",
        )
        pr_gate = workspace / "pr" / "scripts" / "dogma_cleanup_gate.py"
        pr_gate.write_text("raise SystemExit(0)\n", encoding="utf-8")
        pr_fixture_dir = workspace / "pr" / "scripts" / "fixtures" / "dogma_cleanup_gate"
        pr_fixture_dir.mkdir(parents=True, exist_ok=True)
        base_test = (
            workspace
            / "dogma-gate-source"
            / "scripts"
            / "tests"
            / "dogma_cleanup_gate_test.py"
        )
        base_test.parent.mkdir(parents=True, exist_ok=True)
        base_test.write_text(
            "from pathlib import Path\n"
            "import os\n"
            "Path(__file__).with_name('base-ran.txt').write_text(\n"
            "    os.environ['DOGMA_GATE_SCRIPT'] + '\\n' + os.environ['DOGMA_GATE_FIXTURE_DIR'],\n"
            "    encoding='utf-8',\n"
            ")\n",
            encoding="utf-8",
        )

        result = self.run_workflow_step_body(body, workspace)

        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertFalse((pr_test.parent / "pr-ran.txt").exists())
        marker = (base_test.parent / "base-ran.txt").read_text(encoding="utf-8")
        self.assertIn(str(pr_gate), marker)
        self.assertIn(str(pr_fixture_dir), marker)

    def run_unsignaled_source_cleanup(
        self, rel_path: str, deleted_line: str
    ) -> subprocess.CompletedProcess[str]:
        repo = self.create_repo()
        manifest_paths = self.write_workspace_manifest(repo, rel_path)

        source_path = repo / rel_path
        source_path.parent.mkdir(parents=True, exist_ok=True)
        source_path.write_text(
            deleted_line + 'pub fn canonical_owner() -> &\'static str { "new" }\n',
            encoding="utf-8",
        )
        require_success(["git", "add", *manifest_paths, rel_path], repo)
        require_success(["git", "commit", "-q", "-m", "base"], repo)
        require_success(["git", "checkout", "-q", "-b", "pr"], repo)

        source_path.write_text(
            'pub fn canonical_owner() -> &\'static str { "new" }\n',
            encoding="utf-8",
        )
        require_success(["git", "add", rel_path], repo)
        require_success(["git", "commit", "-q", "-m", "delete old path"], repo)
        head_sha = require_success(["git", "rev-parse", "HEAD"], repo)

        event_path = self.github_event_path(head_sha)
        return run_gate(
            [
                "--repo",
                str(repo),
                "--base",
                "main",
                "--github-event",
                str(event_path),
            ]
        )

    def run_unsignaled_path_change(
        self, rel_path: str, base_text: str, pr_text: str
    ) -> subprocess.CompletedProcess[str]:
        repo = self.create_repo()

        changed_path = repo / rel_path
        changed_path.parent.mkdir(parents=True, exist_ok=True)
        changed_path.write_text(base_text, encoding="utf-8")
        require_success(["git", "add", rel_path], repo)
        require_success(["git", "commit", "-q", "-m", "base"], repo)
        require_success(["git", "checkout", "-q", "-b", "pr"], repo)

        changed_path.write_text(pr_text, encoding="utf-8")
        require_success(["git", "add", rel_path], repo)
        require_success(["git", "commit", "-q", "-m", "rewrite gate path"], repo)
        head_sha = require_success(["git", "rev-parse", "HEAD"], repo)

        event_path = self.github_event_path(head_sha)
        return run_gate(
            [
                "--repo",
                str(repo),
                "--base",
                "main",
                "--github-event",
                str(event_path),
            ]
        )

    def create_repo(self) -> Path:
        temp_dir = tempfile.TemporaryDirectory()
        self.addCleanup(temp_dir.cleanup)
        repo = Path(temp_dir.name)
        require_success(["git", "init", "-q"], repo)
        require_success(["git", "config", "user.email", "dogma@example.invalid"], repo)
        require_success(["git", "config", "user.name", "Dogma Gate Test"], repo)
        require_success(["git", "branch", "-M", "main"], repo)
        return repo

    def packet_with_proof(
        self,
        proof: str,
        head_sha: str = "0000000000000000000000000000000000000000",
        search_literal: str = "legacy_endpoint",
        follow_up: str = "none",
        preamble: str = "",
    ) -> Path:
        packet = tempfile.NamedTemporaryFile("w", delete=False, encoding="utf-8")
        self.addCleanup(lambda: Path(packet.name).unlink(missing_ok=True))
        with packet:
            packet.write(
                preamble
                +
                textwrap.dedent(
                    f"""\
                    ## Review Readiness Packet

                    Current head SHA: {head_sha}

                    Command output:

                    ```text
                    $ make dogma-cleanup-gate
                    expected failure
                    ```

                    ## Old Path Amputation Proof

                    | Old path | Classification | Search literal | Mechanical proof | Follow-up |
                    | --- | --- | --- | --- | --- |
                    | `legacy_endpoint` | made uncallable at boundary | `{search_literal}` | {proof} | {follow_up} |

                    ## Complexity Delta

                    - Docs/scripts/tests: 1 file.
                    - Non-test implementation: 0 files.
                    - Generated/schema churn: 0 files.

                    ## Sample Passing Old Path Amputation Proof

                    | Old path | Classification | Search literal | Mechanical proof | Follow-up |
                    | --- | --- | --- | --- | --- |
                    | `old/session-route` | deleted | `old/session-route` | No production references remain. | none |

                    ## Sample Failing Old Path Amputation Proof

                    | Old path | Classification | Search literal | Mechanical proof | Follow-up |
                    | --- | --- | --- | --- | --- |
                    | `old/session-route` | wrapped | `old/session-route` | A preferred path exists beside the old route. | none |
                    """
                )
            )
        return Path(packet.name)

    def github_event_path(self, head_sha: str) -> Path:
        event_file = tempfile.NamedTemporaryFile("w", delete=False, encoding="utf-8")
        self.addCleanup(lambda: Path(event_file.name).unlink(missing_ok=True))
        event_path = Path(event_file.name)
        with event_file:
            json.dump(
                {
                    "pull_request": {
                        "title": "Cleanup runtime path",
                        "body": textwrap.dedent(
                            """\
                            ## Summary

                            Dogma cleanup PR: no

                            - Moves callers to the typed owner without using a dogma label.
                            """
                        ),
                        "labels": [],
                        "head": {"sha": head_sha},
                    }
                },
                event_file,
            )
        return event_path

    def assert_stable_signal_required(
        self,
        result: subprocess.CompletedProcess[str],
        rel_path: str,
        reason: str = "ordinary source path",
    ) -> None:
        self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("Dogma cleanup gate required by stable diff signal", result.stdout)
        self.assertIn(rel_path, result.stdout)
        self.assertIn(reason, result.stdout)
        self.assertIn("Review Readiness Packet is missing required heading", result.stderr)

    def write_workspace_manifest(self, repo: Path, rel_path: str) -> list[str]:
        if "/src/" not in rel_path:
            return []
        member = rel_path.split("/src/", 1)[0]
        if member.startswith(("sdks/", "specs/")):
            return []
        (repo / "Cargo.toml").write_text(
            textwrap.dedent(
                f"""\
                [workspace]
                members = ["{member}"]
                """
            ),
            encoding="utf-8",
        )
        return ["Cargo.toml"]

    def workflow_step_body(
        self, step_name: str, workflow_path: str = ".github/workflows/ci.yml"
    ) -> str:
        workflow = (WORKFLOW_EXEC_ROOT / workflow_path).read_text(encoding="utf-8")
        lines = workflow.splitlines()
        for index, line in enumerate(lines):
            if line.strip() != f"- name: {step_name}":
                continue
            for run_index in range(index + 1, len(lines)):
                if lines[run_index].strip() != "run: |":
                    continue
                run_indent = len(lines[run_index]) - len(lines[run_index].lstrip())
                block_prefix = " " * (run_indent + 2)
                body_lines: list[str] = []
                for body_line in lines[run_index + 1 :]:
                    if not body_line.strip():
                        body_lines.append("")
                    elif body_line.startswith(block_prefix):
                        body_lines.append(body_line[len(block_prefix) :])
                    else:
                        break
                return "\n".join(body_lines)
        raise AssertionError(f"workflow step not found: {step_name}")

    def run_workflow_step_body(
        self, body: str, workspace: Path
    ) -> subprocess.CompletedProcess[str]:
        env = os.environ.copy()
        env["GITHUB_WORKSPACE"] = str(workspace)
        return subprocess.run(
            ["bash", "-e", "-u", "-o", "pipefail", "-c", body],
            cwd=REPO_ROOT,
            env=env,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )


if __name__ == "__main__":
    unittest.main()
