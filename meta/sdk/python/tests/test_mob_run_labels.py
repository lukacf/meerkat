"""Mock-RPC tests for the mob/run label sidecar SDK methods."""

import pytest

from .test_rpc_method_names import make_mock_mob_handle


@pytest.mark.asyncio
async def test_set_mob_labels_rpc_name_and_params():
    handle, calls = make_mock_mob_handle()
    await handle.set_mob_labels({"repo": "agents", "env": "dev"})
    assert calls[0][0] == "meta/mob_labels/set"
    assert calls[0][1] == {"labels": {"repo": "agents", "env": "dev"}}


@pytest.mark.asyncio
async def test_set_mob_labels_empty_clears():
    handle, calls = make_mock_mob_handle()
    await handle.set_mob_labels({})
    assert calls[0][0] == "meta/mob_labels/set"
    assert calls[0][1] == {"labels": {}}


@pytest.mark.asyncio
async def test_get_mob_labels_parses_envelope():
    handle, calls = make_mock_mob_handle({
        "meta/mob_labels/get": {"labels": {"repo": "agents", "env": "dev"}}
    })
    result = await handle.get_mob_labels()
    assert calls[0][0] == "meta/mob_labels/get"
    assert result == {"repo": "agents", "env": "dev"}


@pytest.mark.asyncio
async def test_get_mob_labels_missing_returns_empty():
    handle, _ = make_mock_mob_handle({"meta/mob_labels/get": {}})
    result = await handle.get_mob_labels()
    assert result == {}


@pytest.mark.asyncio
async def test_get_mob_labels_non_dict_returns_empty():
    handle, _ = make_mock_mob_handle({"meta/mob_labels/get": None})
    result = await handle.get_mob_labels()
    assert result == {}


@pytest.mark.asyncio
async def test_delete_mob_labels_rpc_name():
    handle, calls = make_mock_mob_handle()
    await handle.delete_mob_labels()
    assert calls[0][0] == "meta/mob_labels/delete"
    # delete sends no params
    assert calls[0][1] is None


@pytest.mark.asyncio
async def test_set_run_labels_rpc_name_and_params():
    handle, calls = make_mock_mob_handle()
    await handle.set_run_labels("run-123", {"customer": "acme"})
    assert calls[0][0] == "meta/run_labels/set"
    assert calls[0][1] == {"run_id": "run-123", "labels": {"customer": "acme"}}


@pytest.mark.asyncio
async def test_get_run_labels_rpc_name_and_params():
    handle, calls = make_mock_mob_handle({
        "meta/run_labels/get": {"labels": {"trace_id": "abc"}}
    })
    result = await handle.get_run_labels("run-123")
    assert calls[0][0] == "meta/run_labels/get"
    assert calls[0][1] == {"run_id": "run-123"}
    assert result == {"trace_id": "abc"}


@pytest.mark.asyncio
async def test_get_run_labels_missing_returns_empty():
    handle, _ = make_mock_mob_handle({"meta/run_labels/get": {}})
    result = await handle.get_run_labels("run-123")
    assert result == {}


@pytest.mark.asyncio
async def test_delete_run_labels_rpc_name_and_params():
    handle, calls = make_mock_mob_handle()
    await handle.delete_run_labels("run-123")
    assert calls[0][0] == "meta/run_labels/delete"
    assert calls[0][1] == {"run_id": "run-123"}
