from pathlib import Path
import tomllib


def test_phase1_release_parity_python_sdk_metadata_present():
    pyproject = Path(__file__).resolve().parents[1] / "pyproject.toml"
    data = tomllib.loads(pyproject.read_text())

    assert data["project"]["name"] == "meerkat-sdk"
    assert data["project"]["version"]
    assert data["project"]["requires-python"].startswith(">=")
