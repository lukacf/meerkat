"""Tests for MeerkatClient binary discovery and download fallback behavior."""

from pathlib import Path
from unittest.mock import AsyncMock, patch

import pytest

from meerkat.client import MeerkatClient
from meerkat.errors import MeerkatError


@pytest.mark.asyncio
async def test_override_binary_path_is_honored(monkeypatch, tmp_path: Path):
    fake_binary = tmp_path / "meerkat-rpc"
    fake_binary.write_text("binary-placeholder")
    monkeypatch.setenv("MEERKAT_BIN_PATH", str(fake_binary))

    client = MeerkatClient()
    path, use_legacy = await client._resolve_rkat_binary("rkat-rpc")

    assert path == str(fake_binary)
    assert not use_legacy


@pytest.mark.asyncio
async def test_default_path_download_fallback(monkeypatch):
    monkeypatch.delenv("MEERKAT_BIN_PATH", raising=False)
    client = MeerkatClient()

    with patch("meerkat.client.shutil.which", return_value=None), patch.object(
        MeerkatClient,
        "_download_rkat_rpc_binary",
        new=AsyncMock(return_value="/tmp/meerkat-rpc"),
    ):
        path, use_legacy = await client._resolve_rkat_binary("rkat-rpc")

    assert path == "/tmp/meerkat-rpc"
    assert not use_legacy


@pytest.mark.asyncio
async def test_default_path_legacy_fallback_to_rkat(monkeypatch):
    monkeypatch.delenv("MEERKAT_BIN_PATH", raising=False)
    client = MeerkatClient()

    def which(command: str) -> str:
        if command == "rkat-rpc":
            return None
        if command == "rkat":
            return "/usr/local/bin/rkat"
        return None

    with patch("meerkat.client.shutil.which", side_effect=which), patch.object(
        MeerkatClient,
        "_download_rkat_rpc_binary",
        new=AsyncMock(side_effect=MeerkatError("BINARY_DOWNLOAD_FAILED", "missing")),
    ):
        path, use_legacy = await client._resolve_rkat_binary("rkat-rpc")

    assert path == "rkat"
    assert use_legacy


def test_unsupported_platform_rejected():
    with patch("meerkat.client.platform.system", return_value="weird-platform"), patch(
        "meerkat.client.platform.machine", return_value="weird-arch"
    ):
        with pytest.raises(MeerkatError):
            MeerkatClient._platform_target()
