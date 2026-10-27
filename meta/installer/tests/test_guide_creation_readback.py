"""Read existing creation correlations without creating or replaying work."""
from contextlib import asynccontextmanager
from types import SimpleNamespace
import asyncio
import copy
import sqlite3

import httpx
import pytest

from launchkit import server
from launchkit.guide import GuideService
from launchkit.guide_operations import GuideOperations
from launchkit.guide_store import GuideStore
from launchkit.implementation import NoApplicationBuild


REQUEST_A = "cd20e4ba-618c-42e2-b40a-07d1628f1a53"
REQUEST_B = "36d4f452-fc5d-4177-a3e9-3b162f439ccd"
UNKNOWN = "11702c20-443c-487e-bf87-5a5098316e8a"


@pytest.fixture
def creation_service(tmp_path, monkeypatch):
    from launchkit import guide as module

    class NoNativeEntry:
        async def message(self, *args, **kwargs):
            pytest.fail("A creation readback must not enter the setup agent.")

    def no_application(_state):
        raise NoApplicationBuild("No application build")

    monkeypatch.setattr(module, "planner_capability", lambda: {
        "available": True, "provider": "openai", "reason": "Inert fixture"})
    store = GuideStore(tmp_path / "saved")
    service = GuideService(store, NoNativeEntry(), implementation_evidence_provider=no_application)
    operations = GuideOperations()
    monkeypatch.setattr(server, "guide", service)
    monkeypatch.setattr(server, "guide_operations", operations)
    monkeypatch.setattr(server, "architect", SimpleNamespace(
        guide_context=None, proposal=None, guide_result=None, message_lock=asyncio.Lock()))
    return service, operations


@asynccontextmanager
async def client():
    # ASGI transport does not enter the server lifespan or start any runtime.
    async with httpx.AsyncClient(transport=httpx.ASGITransport(app=server.app),
                                 base_url="http://127.0.0.1") as value:
        yield value


def test_created_request_absent_store_does_not_create_private_state(tmp_path):
    store = GuideStore(tmp_path / "not-created")
    assert store.created_request(UNKNOWN) is None
    assert not store.root.exists()


@pytest.mark.parametrize("value", [None, 17, True, {}, "not-an-id", "", REQUEST_A.upper(),
                                  REQUEST_A.replace("-", "")])
def test_created_request_invalid_id_never_opens_database(tmp_path, monkeypatch, value):
    store = GuideStore(tmp_path / "not-created")

    def forbidden_database():
        pytest.fail("An invalid creation request ID must not access the database.")

    monkeypatch.setattr(store, "_database", forbidden_database)
    with pytest.raises(ValueError, match="Invalid setup creation request ID"):
        store.created_request(value)
    assert not store.root.exists()


def test_created_request_reads_exact_current_mapping_and_returns_detached_body(creation_service):
    service, _operations = creation_service
    first = service.store.create(service.new(), request_id=REQUEST_A)
    other = service.store.create(service.new(), request_id=REQUEST_B)
    latest = service.store.save({**first, "summary": "My later saved context"}, first["version"])
    assert service.store.created_request(REQUEST_A) == latest
    assert service.store.created_request(REQUEST_B) == other
    recovered = service.store.created_request(REQUEST_A)
    recovered["summary"] = "Caller-only change"
    assert service.store.created_request(REQUEST_A) == latest
    assert service.store.created_request(first["id"]) is None
    assert len(service.store.list()) == 2


def test_created_request_legacy_store_does_not_create_correlation_table(creation_service):
    service, _operations = creation_service
    first = service.store.create(service.new())
    path = service.store.root / "guided-setups.sqlite3"
    with sqlite3.connect(path) as db:
        before = db.execute("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name").fetchall()
    assert service.store.created_request(UNKNOWN) is None
    with sqlite3.connect(path) as db:
        after = db.execute("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name").fetchall()
    assert before == after == [("guided_setups",)]
    assert service.store.get(first["id"]) == first


@pytest.mark.asyncio
async def test_creation_readback_absent_request_does_not_read_or_enter_operation(creation_service, monkeypatch):
    service, operations = creation_service

    async def forbidden_read(*args):
        pytest.fail("An unconfirmed creation must not read or enter a guide operation.")

    monkeypatch.setattr(operations, "read", forbidden_read)
    async with client() as http:
        response = await http.get(f"/api/guide/creation/{UNKNOWN}")
    assert response.status_code == 404
    assert response.json() == {"detail": "This setup creation is not yet confirmed. Check again before starting another setup."}
    assert not service.store.root.exists()
    assert operations.tasks == {}


@pytest.mark.asyncio
@pytest.mark.parametrize("value", ["not-an-id", REQUEST_A.upper(), REQUEST_A.replace("-", "")])
async def test_creation_readback_invalid_request_is_refused_before_lookup(creation_service, monkeypatch, value):
    service, operations = creation_service

    def forbidden_database():
        pytest.fail("A malformed creation readback must not access private state.")

    async def forbidden_read(*args):
        pytest.fail("A malformed creation readback must not read a guide operation.")

    monkeypatch.setattr(service.store, "_database", forbidden_database)
    monkeypatch.setattr(operations, "read", forbidden_read)
    async with client() as http:
        response = await http.get(f"/api/guide/creation/{value}")
    assert response.status_code == 422
    assert response.json() == {"detail": "Invalid setup creation request ID."}
    assert not service.store.root.exists()


@pytest.mark.asyncio
async def test_creation_readback_uses_current_guide_operation_and_input_surface(creation_service):
    service, operations = creation_service
    first = service.store.create(service.new(), request_id=REQUEST_A)
    other = service.store.create(service.new(), request_id=REQUEST_B)
    latest = service.store.save({**first, "summary": "Saved after creation"}, first["version"])
    operation = {"request_id": "a" * 64, "version": first["version"], "status": "completed",
                 "started_at": "2026-10-10T00:00:00+00:00",
                 "request": {"version": first["version"], "action": "answer", "selected": [],
                             "text": "Keep my original task", "stage": None, "blueprint": None},
                 "detail": "Your response is saved."}
    service.store.save_operation(first["id"], operation)
    async with client() as http:
        response = await http.get(f"/api/guide/creation/{REQUEST_A}")
        regular = await http.get(f"/api/guide/{first['id']}")
        other_response = await http.get(f"/api/guide/creation/{REQUEST_B}")
        wrong = await http.get(f"/api/guide/creation/{first['id']}")
    assert response.status_code == regular.status_code == other_response.status_code == 200
    assert response.json() == regular.json()
    recovered = response.json()
    assert (recovered["id"], recovered["version"], recovered["summary"]) == (
        first["id"], latest["version"], latest["summary"])
    assert recovered["operation"] == operation
    assert recovered["input_surface"] == service.input_surface()
    assert other_response.json()["id"] == other["id"]
    assert "operation" not in other_response.json()
    assert wrong.status_code == 404 and "not yet confirmed" in wrong.json()["detail"]
    assert operations.tasks == {} and len(service.store.list()) == 2
    assert service.store.get(first["id"]) == latest


@pytest.mark.asyncio
async def test_creation_readback_preserves_existing_orphaned_receipt_reconciliation(creation_service):
    service, operations = creation_service
    first = service.store.create(service.new(), request_id=REQUEST_A)
    request = {"version": first["version"], "action": "answer", "selected": ["personal"],
               "text": "Original saved answer", "stage": None, "blueprint": None}
    operation = {"request_id": "b" * 64, "version": first["version"], "status": "running",
                 "request": copy.deepcopy(request), "detail": "Entered before host restart"}
    service.store.save_operation(first["id"], operation)
    async with client() as http:
        response = await http.get(f"/api/guide/creation/{REQUEST_A}")
    assert response.status_code == 200
    recovered = response.json()["operation"]
    assert recovered["status"] == "interrupted"
    assert (recovered["request_id"], recovered["version"], recovered["request"]) == (
        operation["request_id"], first["version"], request)
    assert service.store.operation(first["id"]) == recovered
    assert service.store.get(first["id"]) == first
    assert operations.tasks == {} and len(service.store.list()) == 1
