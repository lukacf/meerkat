"""A product package carries meta-forge executable, as it carries start.sh, and its README names it.

Inert product inputs only: no SDK, gateway, server or native process runs.
"""
from pathlib import Path
import stat

from test_distribution import bundle_source  # noqa: F401
from test_meta_toolkit_runtime_selection import no_product_execution, product  # noqa: F401
from test_packaged_ordinary_gateway import fresh_entry, packaged  # noqa: F401

LAUNCHER = Path(__file__).resolve().parents[1] / "meta-forge"


def test_the_product_package_restores_the_launcher_mode(product, bundle_source, tmp_path, monkeypatch, fresh_entry):
    from launchkit import product_distribution
    extracted, _, _ = packaged(product, bundle_source, monkeypatch, tmp_path, gateway=False)
    launcher = extracted / "meta-forge"
    # ZIP extraction need not keep modes; verification restores the recorded ones.
    launcher.chmod(0o644)
    data = product_distribution.verify(extracted)
    assert data["file_modes"]["meta-forge"] == data["file_modes"]["start.sh"] == 0o755
    assert data["file_modes"]["README.md"] == 0o644
    assert stat.S_IMODE(launcher.stat().st_mode) == 0o755
    assert launcher.read_bytes() == LAUNCHER.read_bytes()
    # The package's generated README names it.
    assert "run `./meta-forge --help` here." in (extracted / "README.md").read_text()
