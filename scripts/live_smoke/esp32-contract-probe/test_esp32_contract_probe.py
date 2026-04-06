#!/usr/bin/env python3

from __future__ import annotations

import tempfile
import unittest
from pathlib import Path

import sys


THIS_DIR = Path(__file__).resolve().parent
if str(THIS_DIR) not in sys.path:
    sys.path.insert(0, str(THIS_DIR))

import esp32_contract_probe as probe


class MarkerParserTests(unittest.TestCase):
    def test_extract_markers_ignores_non_marker_lines(self) -> None:
        markers = probe.extract_markers(
            "\n".join(
                [
                    "hello",
                    "MKT:BOOT:OK lane=host-sim",
                    "noise MKT:WIFI:OK ssid=\"x\"",
                    "MKT:TIME:OK",
                ]
            )
        )
        self.assertEqual(markers, ["MKT:BOOT:OK", "MKT:WIFI:OK", "MKT:TIME:OK"])

    def test_validate_required_markers_accepts_extra_stream_chunks(self) -> None:
        markers = [
            "MKT:BOOT:OK",
            "MKT:WIFI:OK",
            "MKT:TIME:OK",
            "MKT:TLS:OK",
            "MKT:PROVIDER:STREAM_START",
            "MKT:PROVIDER:CHUNK",
            "MKT:PROVIDER:CHUNK",
            "MKT:PROVIDER:STREAM_DONE",
            "MKT:SINGLE_NODE:PASS",
        ]
        probe.validate_required_markers(markers, probe.SINGLE_NODE_MARKERS)

    def test_validate_required_markers_rejects_missing_step(self) -> None:
        with self.assertRaises(probe.MarkerValidationError):
            probe.validate_required_markers(
                [
                    "MKT:HOST_CORE:FACTORY_OK",
                    "MKT:HOST_CORE:RUNTIME_OK",
                    "MKT:HOST_CORE:PASS",
                ],
                probe.HOST_CORE_MARKERS,
            )


class BoardParsingTests(unittest.TestCase):
    def test_choose_port_from_candidates_prefers_cu_variant_for_same_device(self) -> None:
        chosen = probe.choose_port_from_candidates(
            ["/dev/tty.usbmodem101", "/dev/cu.usbmodem101"]
        )
        self.assertEqual(chosen, "/dev/cu.usbmodem101")

    def test_parse_board_info_extracts_expected_fields(self) -> None:
        output = "\n".join(
            [
                "Detecting chip type... ESP32-S3",
                "Connected to ESP32-S3 on /dev/cu.usbmodem101:",
                "Chip type:          ESP32-S3 (QFN56) (revision v0.2)",
                "Features:           Wi-Fi, BT 5 (LE), Dual Core + LP Core, 240MHz, Embedded PSRAM 8MB (AP_3v3)",
                "Crystal frequency:  40MHz",
                "USB mode:           USB-Serial/JTAG",
                "MAC:                28:37:2f:88:d5:48",
                "Detected flash size: 16MB",
            ]
        )
        info = probe.parse_board_info(output, "/dev/cu.usbmodem101")
        self.assertEqual(info["flash_size"], "16MB")
        self.assertEqual(info["psram_mb"], 8)
        self.assertEqual(info["mac"], "28:37:2f:88:d5:48")


class ReportTests(unittest.TestCase):
    def test_write_marker_report_records_profile(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            artifact_dir = Path(temp_dir)
            run_log = artifact_dir / "run.log"
            run_log.write_text(
                "\n".join(
                    [
                        "MKT:HOST_CORE:FACTORY_OK",
                        "MKT:HOST_CORE:PROVIDER_OK",
                        "MKT:HOST_CORE:RUNTIME_OK",
                        "MKT:HOST_CORE:PASS",
                    ]
                )
                + "\n",
                encoding="utf-8",
            )
            context = probe.RunContext(artifact_dir=artifact_dir, run_log=run_log, dry_run=False)
            report = probe.write_marker_report(
                context,
                "host-core-check",
                probe.HOST_CORE_MARKERS,
                run_log,
            )
            self.assertTrue(report["ok"])
            self.assertEqual(report["profile"], "host-core-check")
            self.assertTrue((artifact_dir / "marker_report.json").exists())

    def test_classify_meerkat_baseline_results_detects_expected_blockers(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            artifact_dir = Path(temp_dir)
            run_log = artifact_dir / "run.log"
            run_log.write_text("", encoding="utf-8")
            commands_dir = artifact_dir / "commands"
            commands_dir.mkdir(parents=True, exist_ok=True)

            msrv_log = commands_dir / "01-facade_msrv_check.log"
            msrv_log.write_text(
                "error: rustc 1.93.0-nightly is not supported\nmeerkat@0.5.1 requires rustc 1.94.0\n",
                encoding="utf-8",
            )
            target_log = commands_dir / "02-target_openai_build_std_check.log"
            target_log.write_text(
                "error[E0432]: unresolved import `libc::siginfo_t`\nerror: could not compile `signal-hook-registry` (lib)\n",
                encoding="utf-8",
            )
            ring_log = commands_dir / "03-ring_tree.log"
            ring_log.write_text("ring v0.17.14\n", encoding="utf-8")

            context = probe.RunContext(artifact_dir=artifact_dir, run_log=run_log, dry_run=False)
            summary = probe.classify_meerkat_baseline_results(
                context,
                [
                    {
                        "label": "facade_msrv_check",
                        "log": "commands/01-facade_msrv_check.log",
                    },
                    {
                        "label": "target_openai_build_std_check",
                        "log": "commands/02-target_openai_build_std_check.log",
                    },
                    {
                        "label": "ring_tree",
                        "log": "commands/03-ring_tree.log",
                        "exit_code": 0,
                    },
                ],
            )

            blocker_kinds = {item["kind"] for item in summary["blockers"]}
            self.assertIn("msrv_mismatch", blocker_kinds)
            self.assertIn("tokio_signal_registry_espidf_mismatch", blocker_kinds)
            self.assertIn("rustls_ring_present", blocker_kinds)

    def test_classify_meerkat_baseline_results_detects_expected_blockers(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            artifact_dir = Path(temp_dir)
            run_log = artifact_dir / "run.log"
            run_log.write_text("", encoding="utf-8")
            commands_dir = artifact_dir / "commands"
            commands_dir.mkdir(parents=True, exist_ok=True)

            msrv_log = commands_dir / "01-facade_msrv_check.log"
            msrv_log.write_text(
                "error: rustc 1.93.0-nightly is not supported\nmeerkat@0.5.1 requires rustc 1.94.0\n",
                encoding="utf-8",
            )
            target_log = commands_dir / "02-target_openai_build_std_check.log"
            target_log.write_text(
                "error[E0432]: unresolved import `libc::siginfo_t`\nerror: could not compile `signal-hook-registry` (lib)\n",
                encoding="utf-8",
            )
            ring_log = commands_dir / "03-ring_tree.log"
            ring_log.write_text("ring v0.17.14\n", encoding="utf-8")

            context = probe.RunContext(artifact_dir=artifact_dir, run_log=run_log, dry_run=False)
            summary = probe.classify_meerkat_baseline_results(
                context,
                [
                    {
                        "label": "facade_msrv_check",
                        "log": "commands/01-facade_msrv_check.log",
                    },
                    {
                        "label": "target_openai_build_std_check",
                        "log": "commands/02-target_openai_build_std_check.log",
                    },
                    {
                        "label": "ring_tree",
                        "log": "commands/03-ring_tree.log",
                        "exit_code": 0,
                    },
                ],
            )

            blocker_kinds = {item["kind"] for item in summary["blockers"]}
            self.assertIn("msrv_mismatch", blocker_kinds)
            self.assertIn("tokio_signal_registry_espidf_mismatch", blocker_kinds)
            self.assertIn("rustls_ring_present", blocker_kinds)


class EnvironmentTests(unittest.TestCase):
    def test_sanitized_subprocess_env_removes_python_virtualenv_state(self) -> None:
        env = probe.sanitized_subprocess_env(
            {
                "PATH": "/tmp/bin",
                "VIRTUAL_ENV": "/tmp/venv",
                "VIRTUAL_ENV_DISABLE_PROMPT": "1",
                "__PYVENV_LAUNCHER__": "/tmp/python3",
                "PYTHONEXECUTABLE": "/tmp/python3",
                "PYTHONHOME": "/tmp/home",
                "PYTHONPATH": "/tmp/path",
                "UNCHANGED": "ok",
            }
        )
        self.assertEqual(env["PATH"], "/tmp/bin")
        self.assertEqual(env["UNCHANGED"], "ok")
        self.assertNotIn("VIRTUAL_ENV", env)
        self.assertNotIn("VIRTUAL_ENV_DISABLE_PROMPT", env)
        self.assertNotIn("__PYVENV_LAUNCHER__", env)
        self.assertNotIn("PYTHONEXECUTABLE", env)
        self.assertNotIn("PYTHONHOME", env)
        self.assertNotIn("PYTHONPATH", env)


if __name__ == "__main__":
    unittest.main()
