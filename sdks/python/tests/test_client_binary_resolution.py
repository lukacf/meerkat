"""Tests for MeerkatClient binary discovery and download fallback behavior."""

import os
import urllib.request
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


RELEASE_BASE = "https://github.com/lukacf/meerkat/releases/download"


def test_release_asset_matches_the_published_naming():
    # Release assets carry no "v" before the version; the tag does.
    artifact, url = MeerkatClient._rkat_rpc_release_asset(
        "0.8.50", "x86_64-unknown-linux-gnu", "tar.gz"
    )

    assert artifact == "rkat-rpc-0.8.50-x86_64-unknown-linux-gnu.tar.gz"
    assert url == f"{RELEASE_BASE}/v0.8.50/rkat-rpc-0.8.50-x86_64-unknown-linux-gnu.tar.gz"


@pytest.mark.parametrize(
    ("system", "machine", "expected"),
    [
        ("Linux", "x86_64", "rkat-rpc-0.8.50-x86_64-unknown-linux-gnu.tar.gz"),
        ("Linux", "aarch64", "rkat-rpc-0.8.50-aarch64-unknown-linux-gnu.tar.gz"),
        ("Darwin", "arm64", "rkat-rpc-0.8.50-aarch64-apple-darwin.tar.gz"),
        ("Darwin", "x86_64", "rkat-rpc-0.8.50-x86_64-apple-darwin.tar.gz"),
        ("Windows", "AMD64", "rkat-rpc-0.8.50-x86_64-pc-windows-msvc.zip"),
    ],
)
def test_every_published_target_maps_to_its_release_asset(system, machine, expected):
    target, archive_ext, _binary = MeerkatClient._platform_target(system, machine)
    artifact, url = MeerkatClient._rkat_rpc_release_asset("0.8.50", target, archive_ext)

    assert artifact == expected
    assert url == f"{RELEASE_BASE}/v0.8.50/{expected}"


def test_intel_macos_maps_to_the_x86_64_darwin_asset():
    assert MeerkatClient._platform_target("Darwin", "x86_64") == (
        "x86_64-apple-darwin",
        "tar.gz",
        "rkat-rpc",
    )


@pytest.mark.skipif(
    os.environ.get("MEERKAT_SDK_NETWORK_TESTS") != "1",
    reason="set MEERKAT_SDK_NETWORK_TESTS=1 to check the published asset over the network",
)
def test_published_release_asset_exists():
    _artifact, url = MeerkatClient._rkat_rpc_release_asset(
        "0.8.50", "x86_64-unknown-linux-gnu", "tar.gz"
    )
    request = urllib.request.Request(url, method="HEAD")
    with urllib.request.urlopen(request, timeout=30) as response:
        assert response.status == 200


def test_default_connect_args_do_not_enable_live_transports():
    args = MeerkatClient._build_args(
        False,
        isolated=False,
        realm_id=None,
        instance_id=None,
        realm_backend=None,
        state_root=None,
        context_root=None,
        user_config_root=None,
        live_ws=False,
        live_webrtc=False,
    )

    assert "--live-ws" not in args
    assert "--live-webrtc" not in args


def test_live_ws_connect_args_are_opt_in():
    args = MeerkatClient._build_args(
        False,
        isolated=False,
        realm_id=None,
        instance_id=None,
        realm_backend=None,
        state_root=None,
        context_root=None,
        user_config_root=None,
        live_ws=True,
        live_webrtc=False,
    )

    assert args[:2] == ["--live-ws", "127.0.0.1:0"]


def test_live_webrtc_connect_args_are_opt_in():
    args = MeerkatClient._build_args(
        False,
        isolated=False,
        realm_id=None,
        instance_id=None,
        realm_backend=None,
        state_root=None,
        context_root=None,
        user_config_root=None,
        live_ws=False,
        live_webrtc=True,
    )

    assert args == ["--live-webrtc"]
