"""Adapter ownership controls using synchronous fake calls, without native IO."""
import asyncio
from concurrent.futures import ThreadPoolExecutor
import hashlib
import importlib.util
from pathlib import Path
import sys
import threading
from types import ModuleType

import pytest

from launchkit.templates_v2.console_auth import ConsoleConnectLogin, ISSUER


@pytest.fixture(autouse=True)
def adopted_source_imports(monkeypatch):
    """Isolate uncalled Meta type imports; this is not SDK compatibility proof."""
    source = Path(__file__).parents[1] / "tests/fixtures/retained_login_sdk/host_control.py"
    assert hashlib.sha256(source.read_bytes()).hexdigest() == (
        "bec462e8e0538fb887151d76754998055cbd311b394cfcc30a5225d2cb7ea03f")

    class UncalledDependency:
        def __new__(cls, *args, **kwargs):
            raise AssertionError("No Meta runtime or transport construction is permitted.")

    for package_name in ("meerkat_meta", "meerkat_toolkit"):
        package = ModuleType(package_name)
        package.__path__ = []
        monkeypatch.setitem(sys.modules, package_name, package)
    for name, attribute in (("meerkat_meta._transport", "PersistentTransport"),
                            ("meerkat_meta.runtime", "MetaRuntime")):
        dependency = ModuleType(name)
        setattr(dependency, attribute, UncalledDependency)
        monkeypatch.setitem(sys.modules, name, dependency)

    def load(name, path):
        spec = importlib.util.spec_from_file_location(name, path)
        module = importlib.util.module_from_spec(spec)
        monkeypatch.setitem(sys.modules, name, module)
        spec.loader.exec_module(module)
        return module

    sdk = load("meerkat_toolkit.host_control", source)
    adapter = load("launchkit.templates_v2.native_console_login", Path(__file__).parents[1] /
                   "launchkit/templates_v2/native_console_login.py")
    enrollment, client = fake_types(sdk.ConsoleLoginEnrollment, sdk.PrivateHostClient,
                                    sdk.ConsoleLoginRetirement)
    for name, value in {"NativeConsoleLogin": adapter.NativeConsoleLogin,
                        "NativeConsoleLoginError": adapter.NativeConsoleLoginError,
                        "ConsoleLoginRetirement": sdk.ConsoleLoginRetirement,
                        "Enrollment": enrollment, "Client": client}.items():
        monkeypatch.setitem(globals(), name, value)
    yield


def fake_types(EnrollmentBase, ClientBase, Retirement):
    class Enrollment(EnrollmentBase):
        def __init__(self):
            # No pipe, native host or SDK reader. Only synchronous call custody is
            # exercised here; the adopted SDK owns its separate protocol controls.
            self.result_entered = threading.Event()
            self.result_release = threading.Event()
            self.retire_entered = threading.Event()
            self.retire_returned = threading.Event()
            self.retire_release = threading.Event()
            self.retire_release.set()
            self.result_error = self.retire_error = None
            self.retirement = Retirement(uncertain=False)
            self.release_result_on_retire = True
            self.result_calls = self.retire_calls = 0
            self.result_thread = self.retire_thread = None

        @property
        def enrollment_id(self):
            return 17

        def result(self):
            self.result_calls += 1
            self.result_thread = threading.get_ident()
            self.result_entered.set()
            if not self.result_release.wait(5):
                raise AssertionError("Fixture result was not released.")
            if self.result_error is not None:
                raise self.result_error

        def retire(self, *, timeout=None):
            self.retire_calls += 1
            self.retire_thread = threading.get_ident()
            self.retire_entered.set()
            if not self.retire_release.wait(5):
                raise AssertionError("Fixture retirement was not released.")
            if self.release_result_on_retire:
                self.result_release.set()
            if self.retire_error is not None:
                raise self.retire_error
            self.retire_returned.set()
            return self.retirement


    class Client(ClientBase):
        def __init__(self, enrollment):
            self.enrollment = enrollment
            self.begin_entered = threading.Event()
            self.begin_release = threading.Event()
            self.begin_release.set()
            self.begin_error = None
            self.selections = []
            self.close_calls = 0

        def begin_console_login(self, selection, *, timeout=None):
            self.selections.append(selection)
            self.begin_entered.set()
            if not self.begin_release.wait(5):
                raise AssertionError("Fixture begin was not released.")
            if self.begin_error is not None:
                raise self.begin_error
            return self.enrollment

        def close(self):
            self.close_calls += 1
            raise AssertionError("The adapter cannot close its shared client.")

    return Enrollment, Client


def fixture():
    current = {"login": True, "application": True}
    login = ConsoleConnectLogin("actual-principal", "original-signed-sid",
        1893456000123, "http://127.0.0.1:8787", lambda: current["login"])
    enrollment = Enrollment()
    client = Client(enrollment)
    owner = NativeConsoleLogin(client, login, issuer=ISSUER,
        is_current=lambda: current["application"])
    return owner, client, enrollment, current, login


async def entered(event):
    # Avoid consuming a worker, especially in the default-capacity control.
    async with asyncio.timeout(30):
        while not event.is_set():
            await asyncio.sleep(0.001)


def test_exact_login_labels_original_handle_and_owned_worker_close():
    async def exercise():
        owner, client, enrollment, _, login = fixture()
        try:
            assert owner._login is login
            assert (owner.issuer, owner.subject, owner.login_scope_id, owner.expires_at_ms,
                    owner.origin) == (ISSUER, login.principal, login.login_scope_id,
                                      login.expires_at_ms, login.origin)
            with pytest.raises(AttributeError):
                owner.login_scope_id = "replacement"
            with pytest.raises(NativeConsoleLoginError, match="unavailable") as error:
                await owner.ready()
            assert error.value.code == "unavailable"
            assert not client.selections
            owner.start()
            enrollment.result_release.set()
            assert await owner.ready() is enrollment
            assert await owner.ready() is enrollment
            selection, = client.selections
            assert (selection.issuer, selection.subject, selection.login_scope_id,
                    selection.expires_at_ms) == (ISSUER, login.principal,
                                               login.login_scope_id, login.expires_at_ms)
            with pytest.raises(NativeConsoleLoginError):
                owner.start()
            assert await owner.close() is enrollment.retirement
            assert await owner.close() is enrollment.retirement
            assert enrollment.result_calls == enrollment.retire_calls == 1
            assert client.close_calls == 0
            assert not owner.is_current()
            assert all(not thread.is_alive() for pool in (owner._calls, owner._retirement_calls)
                       for thread in pool._threads)
        finally:
            enrollment.result_release.set()
            enrollment.retire_release.set()
            await owner.close()
    asyncio.run(exercise())


def test_cancelled_ready_retains_entered_begin_and_retires_same_handle():
    async def exercise():
        owner, client, enrollment, _, _ = fixture()
        client.begin_release.clear()
        owner.start()
        waiter = asyncio.create_task(owner.ready())
        try:
            await entered(client.begin_entered)
            waiter.cancel()
            with pytest.raises(asyncio.CancelledError):
                await waiter
            owner.revoke()
            assert not owner.is_current()
            retirement = asyncio.create_task(owner.retire("logout"))
            await asyncio.sleep(0)
            assert not retirement.done()
            assert enrollment.retire_calls == 0
            client.begin_release.set()
            assert await retirement is enrollment.retirement
            assert owner._enrollment is enrollment
            assert len(client.selections) == enrollment.retire_calls == enrollment.result_calls == 1
            assert not owner._begin_task.cancelled()
            assert not owner._result_task.cancelled()
        finally:
            client.begin_release.set()
            enrollment.result_release.set()
            enrollment.retire_release.set()
            await owner.close()
    asyncio.run(exercise())


def test_cancelled_result_wait_and_retire_wait_keep_original_join():
    async def exercise():
        owner, client, enrollment, _, _ = fixture()
        enrollment.retire_release.clear()
        owner.start()
        waiter = asyncio.create_task(owner.ready())
        try:
            await entered(enrollment.result_entered)
            waiter.cancel()
            with pytest.raises(asyncio.CancelledError):
                await waiter
            retirement = asyncio.create_task(owner.retire("logout"))
            await entered(enrollment.retire_entered)
            retirement.cancel()
            with pytest.raises(asyncio.CancelledError):
                await retirement
            assert not owner._result_task.cancelled()
            assert not owner._retire_task.cancelled()
            closing = asyncio.create_task(owner.close())
            await asyncio.sleep(0)
            assert not closing.done()
            enrollment.retire_release.set()
            assert await closing is enrollment.retirement
            assert len(client.selections) == enrollment.result_calls == enrollment.retire_calls == 1
        finally:
            enrollment.result_release.set()
            enrollment.retire_release.set()
            await owner.close()
    asyncio.run(exercise())


def test_retirement_schedules_with_result_and_default_worker_occupied():
    async def exercise():
        owner, _, enrollment, _, _ = fixture()
        default_entered, default_release = threading.Event(), threading.Event()
        default_pool = ThreadPoolExecutor(max_workers=1)
        asyncio.get_running_loop().set_default_executor(default_pool)
        def hold_default():
            default_entered.set()
            if not default_release.wait(5):
                raise AssertionError("Fixture default worker was not released.")
        held = asyncio.get_running_loop().run_in_executor(None, hold_default)
        owner.start()
        try:
            await entered(default_entered)
            await entered(enrollment.result_entered)
            assert not enrollment.result_release.is_set()
            assert await asyncio.wait_for(owner.retire("logout"), 2) is enrollment.retirement
            assert enrollment.retire_thread != enrollment.result_thread
            assert not default_release.is_set()
        finally:
            default_release.set()
            enrollment.result_release.set()
            enrollment.retire_release.set()
            await held
            await owner.close()
    asyncio.run(exercise())


def test_confirmed_retirement_still_joins_original_result_before_close():
    async def exercise():
        owner, client, enrollment, _, _ = fixture()
        enrollment.release_result_on_retire = False
        owner.start()
        try:
            await entered(enrollment.result_entered)
            closing = asyncio.create_task(owner.close())
            await entered(enrollment.retire_returned)
            assert not closing.done()
            assert not owner._result_task.done()
            assert not owner.is_current()
            enrollment.result_release.set()
            assert await closing is enrollment.retirement
            assert owner._result_task.done()
            assert enrollment.result_calls == enrollment.retire_calls == len(client.selections) == 1
        finally:
            enrollment.result_release.set()
            await owner.close()
    asyncio.run(exercise())


@pytest.mark.parametrize("boundary", ["begin", "result"])
@pytest.mark.parametrize("owner_kind", ["login", "application"])
def test_ready_rechecks_exact_current_owner_after_await(boundary, owner_kind):
    async def exercise():
        owner, client, enrollment, current, _ = fixture()
        if boundary == "begin":
            client.begin_release.clear()
        owner.start()
        waiter = asyncio.create_task(owner.ready())
        try:
            await entered(client.begin_entered if boundary == "begin" else enrollment.result_entered)
            current[owner_kind] = False
            client.begin_release.set()
            enrollment.result_release.set()
            with pytest.raises(NativeConsoleLoginError) as error:
                await waiter
            assert error.value.code == "invalid_binding"
            assert not owner.is_current()
            assert await owner.close() is enrollment.retirement
        finally:
            client.begin_release.set()
            enrollment.result_release.set()
            enrollment.retire_release.set()
            await owner.close()
    asyncio.run(exercise())


@pytest.mark.parametrize("failure", [TimeoutError("private timeout"), EOFError("private EOF")])
def test_original_readiness_failure_survives_late_reply_and_separate_retirement(failure):
    async def exercise():
        owner, client, enrollment, _, _ = fixture()
        enrollment.result_error = failure
        enrollment.result_release.set()
        owner.start()
        try:
            for _ in range(2):
                with pytest.raises(NativeConsoleLoginError) as error:
                    await owner.ready()
                assert error.value.code == "outcome_unknown"
                assert "private" not in str(error.value)
                # Even a fake capable of recovering late cannot make the
                # adapter resend/reobserve its original failed result.
                enrollment.result_error = None
            assert enrollment.result_calls == len(client.selections) == 1
            assert await owner.close() is enrollment.retirement
            with pytest.raises(NativeConsoleLoginError) as error:
                await owner.ready()
            assert error.value.code == "invalid_binding"
        finally:
            await owner.close()
    asyncio.run(exercise())


@pytest.mark.parametrize("failure", ["uncertain", "exception", "invalid_fence", "invalid_drain"])
def test_original_retirement_failure_is_joined_and_never_retried(failure):
    async def exercise():
        owner, client, enrollment, _, _ = fixture()
        if failure == "exception":
            enrollment.retire_error = TimeoutError("private retirement canary")
        else:
            enrollment.retirement = ConsoleLoginRetirement(
                uncertain=failure == "uncertain", fenced=failure != "invalid_fence",
                reservations_drained=failure != "invalid_drain")
        owner.start()
        await entered(enrollment.result_entered)
        for close in (lambda: owner.retire("logout"), owner.close, owner.close):
            with pytest.raises(NativeConsoleLoginError) as error:
                await close()
            assert error.value.code == "cleanup_unconfirmed"
            assert "canary" not in str(error.value)
        assert enrollment.retire_calls == enrollment.result_calls == len(client.selections) == 1
        assert owner._result_task.done()
        assert client.close_calls == 0
    asyncio.run(exercise())


def test_begin_failure_stays_unknown_without_manufacturing_handle_or_retry():
    async def exercise():
        owner, client, enrollment, _, _ = fixture()
        client.begin_error = TimeoutError("private begin canary")
        owner.start()
        with pytest.raises(NativeConsoleLoginError) as error:
            await owner.ready()
        assert error.value.code == "outcome_unknown"
        for _ in range(2):
            with pytest.raises(NativeConsoleLoginError) as error:
                await owner.close()
            assert error.value.code == "cleanup_unconfirmed"
        assert len(client.selections) == 1
        assert enrollment.result_calls == enrollment.retire_calls == client.close_calls == 0
    asyncio.run(exercise())


def test_dormant_close_and_revocation_do_not_start_enrollment():
    async def exercise():
        owner, client, _, _, _ = fixture()
        owner.revoke()
        with pytest.raises(NativeConsoleLoginError):
            owner.start()
        assert await owner.close() is None
        assert await owner.retire("logout") is None
        assert client.selections == []
        assert client.close_calls == 0
    asyncio.run(exercise())


@pytest.mark.parametrize("owner_kind", ["login", "application"])
def test_current_owner_requires_boolean_true_before_admission(owner_kind):
    async def exercise():
        owner, client, _, current, _ = fixture()
        current[owner_kind] = 1
        assert not owner.is_current()
        with pytest.raises(NativeConsoleLoginError):
            owner.start()
        assert await owner.close() is None
        assert client.selections == []
    asyncio.run(exercise())


def test_retiring_original_login_does_not_change_another_login_on_shared_client():
    async def exercise():
        owner, client, original, _, _ = fixture()
        second = Enrollment()
        current = {"value": True}
        login = ConsoleConnectLogin("actual-principal", "different-signed-sid",
            1893456000456, "http://127.0.0.1:8787", lambda: current["value"])
        other = NativeConsoleLogin(client, login, issuer=ISSUER)
        owner.start()
        try:
            await entered(original.result_entered)
            # Both handles belong to this shared client. Change only its next
            # returned fixture handle after the original call has returned.
            client.enrollment = second
            other.start()
            await entered(second.result_entered)
            assert await owner.close() is original.retirement
            assert other.is_current()
            assert second.retire_calls == 0
            second.result_release.set()
            assert await other.ready() is second
            assert [selection.login_scope_id for selection in client.selections] == [
                "original-signed-sid", "different-signed-sid"]
        finally:
            original.result_release.set()
            second.result_release.set()
            await owner.close()
            await other.close()
        assert client.close_calls == 0
    asyncio.run(exercise())
