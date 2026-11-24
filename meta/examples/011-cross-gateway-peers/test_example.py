"""Offline checks: no provider and no gateway process."""
import os
import subprocess
import sys
import unittest
from pathlib import Path

from main import require_signed_loopback


class SetupChecks(unittest.TestCase):
    def test_gateway_not_set(self):
        env = os.environ.copy()
        env.pop('META_GATEWAY_BIN', None)
        run = subprocess.run([sys.executable, str(Path(__file__).with_name('main.py'))],
                             env=env, capture_output=True, text=True, timeout=10)
        self.assertEqual(run.returncode, 2)
        self.assertIn('Set META_GATEWAY_BIN', run.stderr)
        self.assertNotIn('Traceback', run.stderr)

    def test_signed_loopback(self):
        good = {'dialable_address': 'tcp://127.0.0.1:4011', 'pubkey_b64': 'fixture'}
        require_signed_loopback('editor', good)
        for bad in ({}, {**good, 'dialable_address': None}, {**good, 'pubkey_b64': ''},
                    {**good, 'dialable_address': 'tcp://192.0.2.1:4011'}):
            with self.subTest(bad=bad), self.assertRaisesRegex(RuntimeError, 'editor has no signed loopback endpoint'):
                require_signed_loopback('editor', bad)
