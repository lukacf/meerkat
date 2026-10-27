"""Bounded package-resource loading, with the Rust projection as schema owner."""

import hashlib
from importlib.resources import files
import json
from pathlib import Path
from tempfile import TemporaryDirectory
import unittest
from unittest.mock import patch

from meerkat_toolkit.catalog import CatalogResourceError, bundled_frontend_catalog_json


class FrontendCatalogTests(unittest.TestCase):
    def test_installed_resource_is_returned_verbatim_with_its_pin(self):
        root = files("meerkat_toolkit").joinpath("data")
        raw = root.joinpath("frontend_catalog.json").read_bytes()
        pin = root.joinpath("frontend_catalog.sha256").read_text(encoding="ascii")
        self.assertEqual(bundled_frontend_catalog_json().encode("utf-8"), raw)
        self.assertEqual(pin, hashlib.sha256(raw).hexdigest() + "\n")
        # No local schema reconstruction: Rust's drift test validates the records.
        self.assertGreater(len(json.loads(raw)["entries"]), 0)

    def test_missing_damaged_oversized_or_badly_encoded_resources_are_refused(self):
        valid = bundled_frontend_catalog_json().encode("utf-8")
        cases = [
            (None, None),
            (None, hashlib.sha256(valid).hexdigest().encode() + b"\n"),
            (valid, b"invalid\n"),
            (valid, b"0" * 64 + b"\n"),
            (valid, b"A" * 64 + b"\n"),
            (valid, b"0" * 65 + b"\n"),
            (valid + b" ", hashlib.sha256(valid).hexdigest().encode() + b"\n"),
            (b" " * (64 * 1024 + 1), None),
            (b"\xff", None),
        ]
        for data, pin in cases:
            with self.subTest(size=None if data is None else len(data)):
                with TemporaryDirectory() as directory:
                    root = Path(directory)
                    (root / "data").mkdir()
                    if data is not None:
                        (root / "data/frontend_catalog.json").write_bytes(data)
                        if pin is None:
                            pin = hashlib.sha256(data).hexdigest().encode() + b"\n"
                    if pin is not None:
                        (root / "data/frontend_catalog.sha256").write_bytes(pin)
                    with patch("meerkat_toolkit.catalog.files", return_value=root):
                        with self.assertRaises(CatalogResourceError):
                            bundled_frontend_catalog_json()


if __name__ == "__main__":
    unittest.main()
