"""The application's stop deadlines follow the gateway shutdown horizon it reports.

Owned harmless process fixtures and source reads; no native runtime, service
manager or remote destination is exercised.
"""
import asyncio
from pathlib import Path
import re
import signal
from types import SimpleNamespace

import pytest

from launchkit import deployment_deadlines as deadlines
from launchkit import deployment_worker


REPOSITORY = Path(__file__).resolve().parents[2]
GATEWAY_SOURCE = REPOSITORY / "crates/meerkat-meta/src/rpc_gateway.rs"
SDK_TRANSPORT = REPOSITORY / "sdk/python/meerkat_meta/_transport.py"
# Forge's margins around the gateway horizon: the installed SDK's TERM and
# KILL graces, then time for the application to close its adapters.
TRANSPORT_AND_ADAPTER_MARGIN = 10 + 10


def defined_constant(text, pattern, comment):
    """Read one constant definition; commented-out, absent or repeated ones fail closed."""
    found = []
    for line in text.splitlines():
        code = line.split(comment, 1)[0]
        match = re.fullmatch(pattern, code.strip())
        if match is not None:
            found.append(int(match.group(1).replace("_", "")))
    if len(found) != 1:
        raise AssertionError(f"expected exactly one definition, found {len(found)}")
    return found[0]


def gateway_horizon_ms(text=None):
    """GATEWAY_SHUTDOWN_HORIZON_MS as this repository's rpc_gateway advertises it."""
    return defined_constant(GATEWAY_SOURCE.read_text() if text is None else text,
        r"(?:pub(?:\([a-z]+\))?\s+)?const\s+GATEWAY_SHUTDOWN_HORIZON_MS\s*:\s*u64\s*=\s*([0-9][0-9_]*)\s*;", "//")


def sdk_horizon_bound_ms(text=None):
    """The largest advertised horizon the Python SDK accepts."""
    return defined_constant(SDK_TRANSPORT.read_text() if text is None else text,
        r"_MAX_GATEWAY_SHUTDOWN_HORIZON_MS\s*=\s*([0-9][0-9_]*)", "#")


def test_constant_reader_fails_closed_on_missing_commented_or_repeated_definitions():
    source = "// const GATEWAY_SHUTDOWN_HORIZON_MS: u64 = 1;\nconst GATEWAY_SHUTDOWN_HORIZON_MS: u64 = 432_000; // now\n"
    assert gateway_horizon_ms(source) == 432_000
    for broken in ("// const GATEWAY_SHUTDOWN_HORIZON_MS: u64 = 1;\n",
            "const GATEWAY_SHUTDOWN_HORIZON_MS: u64 = 1;\nconst GATEWAY_SHUTDOWN_HORIZON_MS: u64 = 2;\n",
            "const OTHER_HORIZON_MS: u64 = 1;\n"):
        with pytest.raises(AssertionError, match="exactly one definition"):
            gateway_horizon_ms(broken)
    assert sdk_horizon_bound_ms("# _MAX_GATEWAY_SHUTDOWN_HORIZON_MS = 1\n_MAX_GATEWAY_SHUTDOWN_HORIZON_MS = 2_000\n") == 2000


def test_unreported_horizon_is_this_repository_gateway_advertisement():
    # The in-repo gateway's horizon moves Forge's fallback in the same change.
    assert getattr(deadlines, "UNREPORTED_GATEWAY_SHUTDOWN_HORIZON_MS", None) == gateway_horizon_ms()


def test_reported_horizon_bound_is_the_python_sdk_parse_bound():
    assert getattr(deadlines, "MAX_REPORTED_GATEWAY_SHUTDOWN_HORIZON_MS", None) == sdk_horizon_bound_ms()


def observed_stop_window(monkeypatch, supervisor):
    """Run terminate() and return how long it waited for the application parent."""
    process = SimpleNamespace(pid=7781, returncode=None, terminate=lambda: None)
    async def wait(): return process.returncode
    process.wait = wait
    supervisor.child = process
    windows = []
    async def model_wait(awaitable, timeout):
        if getattr(getattr(awaitable, "cr_code", None), "co_name", None) == "parent_exit":
            windows.append(timeout)
            process.returncode = 0
        return await awaitable
    def owned_signal(pid, sig):
        assert pid == process.pid and sig in {signal.SIGTERM, signal.SIGKILL}
    monkeypatch.setattr(deployment_worker.asyncio, "wait_for", model_wait)
    monkeypatch.setattr(deployment_worker.os, "killpg", owned_signal)
    return windows


@pytest.mark.parametrize("reported_ms", [392_000, 432_000], ids=["meta-0.9.4", "meta-0.9.5"])
async def test_application_stop_outlasts_the_gateway_horizon_it_reported(monkeypatch, reported_ms):
    # The application's SDK waits this horizon for native cleanup, then its
    # own TERM and KILL graces. Cutting earlier kills the gateway mid-cleanup.
    supervisor = deployment_worker.Supervisor.__new__(deployment_worker.Supervisor)
    supervisor.gateway_shutdown_horizon_ms = reported_ms
    windows = observed_stop_window(monkeypatch, supervisor)
    await supervisor.terminate()
    assert windows == [reported_ms // 1000 + TRANSPORT_AND_ADAPTER_MARGIN]


async def test_unreported_application_stop_outlasts_this_repository_gateway_horizon(monkeypatch):
    # An application on an SDK without the accessor reports nothing; its
    # gateway is this repository's line or older.
    supervisor = deployment_worker.Supervisor.__new__(deployment_worker.Supervisor)
    windows = observed_stop_window(monkeypatch, supervisor)
    await supervisor.terminate()
    assert windows == [gateway_horizon_ms() // 1000 + TRANSPORT_AND_ADAPTER_MARGIN]


# Enclosing budgets around a horizon of H seconds: the application stop is
# H + 20, the worker's adds its force reap and five seconds, and the OS unit's
# stop timeout adds fifteen more.
def service_budget(horizon_ms):
    return horizon_ms // 1000 + TRANSPORT_AND_ADAPTER_MARGIN + 10 + 5 + 15


@pytest.fixture
def background(tmp_path, monkeypatch, request):
    """A checked background installation before its operating-system unit exists."""
    import plistlib
    import sys
    from launchkit import deployment, deployment_service
    from test_deployment import Guide, Implementation, Service, submit
    platform = request.param
    monkeypatch.setattr(deployment_service, "sys", SimpleNamespace(platform=platform, executable=sys.executable))
    monkeypatch.setenv("HOME", str(tmp_path / "home"))
    Path.home().mkdir()
    guide = Guide()
    async def capability():
        return {"available": True, "manager": "launchd" if platform == "darwin" else "systemd-user"}
    async def prepared():
        held = Service(tmp_path / "server", guide, Implementation(guide),
            service_manager=SimpleNamespace(capability=capability))
        await submit(held, tmp_path / "application", execution_mode="background")
        directory = held._directory(guide.identity)
        deployment.materialize(directory)
        await held.shutdown()
        return directory
    def stop_timeout(content):
        if platform == "darwin":
            return plistlib.loads(content)["ExitTimeOut"]
        return int(re.search(rb"\nTimeoutStopSec=([0-9]+)\n", content).group(1))
    return SimpleNamespace(platform=platform, prepared=prepared, stop_timeout=stop_timeout,
        deployment=deployment, service=deployment_service)


@pytest.mark.parametrize("background", ["darwin", "linux"], indirect=True)
@pytest.mark.parametrize("recorded", [None, 347_000, 432_000], ids=["unreported", "reported-347s", "reported-432s"])
async def test_service_unit_stop_budget_follows_the_recorded_gateway_horizon(background, recorded):
    f = background
    directory = await f.prepared()
    if recorded is not None:
        f.deployment.update_receipt(directory, gateway_shutdown_horizon_ms=recorded)
    service = f.service.prepare_service(directory, f.deployment.read_receipt(directory))
    # A new unit never gets less than the unreported horizon's budget; a
    # longer report widens it.
    expected = max(service_budget(gateway_horizon_ms()), service_budget(recorded) if recorded else 0)
    assert service["stop_timeout_seconds"] == expected
    assert f.stop_timeout(Path(service["unit_path"]).read_bytes()) == expected


@pytest.mark.parametrize("background", ["linux"], indirect=True)
async def test_recorded_unit_budgets_are_exactly_the_derived_ones(background):
    f = background
    directory = await f.prepared()
    f.deployment.update_receipt(directory, gateway_shutdown_horizon_ms=432_000)
    service = f.service.prepare_service(directory, f.deployment.read_receipt(directory))
    unit = Path(service["unit_path"])
    def recorded_with(timeout):
        changed = {**service, "stop_timeout_seconds": timeout}
        content = f.service.unit_bytes(changed)
        unit.write_bytes(content)
        changed["unit_sha256"] = f.service.digest(content)
        f.deployment.update_receipt(directory, service=changed)
        return f.service.verify_service(directory, f.deployment.read_receipt(directory))
    # The legacy 45-second unit, the unit the 347-second pin wrote, and units
    # derived from the recorded or the unreported horizon stay controllable.
    for accepted in (45, 397, service_budget(432_000), service_budget(gateway_horizon_ms())):
        assert recorded_with(accepted)["stop_timeout_seconds"] == accepted
    for refused in (396, 398, service_budget(432_000) + 1, 3600):
        with pytest.raises(Exception, match="budget is unsupported"):
            recorded_with(refused)


DROP_IN = "forge-stop-budget.conf"


def earlier_unit(f, directory):
    """Record the unit an earlier Forge wrote from the 347-second pin."""
    service = f.service.prepare_service(directory, f.deployment.read_receipt(directory))
    earlier = {**service, "stop_timeout_seconds": 397}
    content = f.service.unit_bytes(earlier)
    Path(service["unit_path"]).write_bytes(content)
    earlier["unit_sha256"] = f.service.digest(content)
    f.deployment.update_receipt(directory, service=earlier)
    return earlier, content


@pytest.mark.parametrize("background", ["darwin", "linux"], indirect=True)
@pytest.mark.parametrize("reported", [392_000, 432_000, None], ids=["meta-0.9.4", "meta-0.9.5", "unreported"])
async def test_an_earlier_unit_gets_a_stop_budget_drop_in_for_the_reported_horizon(background, reported):
    f = background
    directory = await f.prepared()
    earlier, content = earlier_unit(f, directory)
    unit = Path(earlier["unit_path"])
    widen = getattr(f.service, "widen_service_stop_budget", None)
    assert widen is not None, "the service owner cannot widen a recorded unit's stop budget"
    reload = widen(directory, reported)
    receipt = f.deployment.read_receipt(directory)
    drop_in = Path(str(unit) + ".d") / DROP_IN
    # The frozen unit and the service identity never change.
    assert unit.read_bytes() == content and receipt["service"] == earlier
    assert f.service.verify_service(directory, receipt) == earlier
    if f.platform == "darwin":
        # launchd reads ExitTimeOut only when it loads the job and has no
        # drop-in: an earlier plist keeps the stop timeout it was installed with.
        assert reload is None and not drop_in.parent.exists() and "service_stop_budget" not in receipt
        return
    # An application that reports nothing is sized for the unreported
    # horizon, which the record names explicitly.
    sized_for = gateway_horizon_ms() if reported is None else reported
    budget = service_budget(sized_for)
    assert reload == ["/usr/bin/systemctl", "--user", "daemon-reload"]
    assert drop_in.read_bytes() == f"[Service]\nTimeoutStopSec={budget}\n".encode()
    assert receipt["service_stop_budget"] == {"gateway_shutdown_horizon_ms": sized_for, "stop_timeout_seconds": budget}
    # A shorter or equal report never narrows or rewrites it. Each readiness
    # reloads the manager again, completing a reload an interrupted run missed.
    before = (drop_in.read_bytes(), f.deployment.read_receipt(directory)["service_stop_budget"])
    for again in (392_000, reported):
        assert widen(directory, again) == reload
    assert (drop_in.read_bytes(), f.deployment.read_receipt(directory)["service_stop_budget"]) == before


@pytest.mark.parametrize("background", ["linux"], indirect=True)
async def test_a_unit_at_the_derived_budget_needs_no_drop_in(background):
    f = background
    directory = await f.prepared()
    service = f.service.prepare_service(directory, f.deployment.read_receipt(directory))
    for unreported in (gateway_horizon_ms(), None):
        assert f.service.widen_service_stop_budget(directory, unreported) is None
    assert not Path(service["unit_path"] + ".d").exists()
    assert "service_stop_budget" not in f.deployment.read_receipt(directory)


@pytest.mark.parametrize("background", ["linux"], indirect=True)
async def test_a_stop_budget_drop_in_is_verified_byte_exact(background):
    f = background
    directory = await f.prepared()
    earlier, _ = earlier_unit(f, directory)
    drop_in = Path(earlier["unit_path"] + ".d") / DROP_IN
    f.service.widen_service_stop_budget(directory, 432_000)
    written = drop_in.read_bytes()
    receipt = f.deployment.read_receipt(directory)
    # Foreign bytes in the owned drop-in control nothing.
    drop_in.write_bytes(b"[Service]\nTimeoutStopSec=3600\n")
    with pytest.raises(Exception, match="definition changed"):
        f.service.verify_service(directory, receipt)
    # Neither does a drop-in this installation never recorded.
    drop_in.write_bytes(written)
    unrecorded = {key: value for key, value in receipt.items() if key != "service_stop_budget"}
    with pytest.raises(Exception, match="definition changed"):
        f.service.verify_service(directory, unrecorded)
    # A record whose drop-in write was interrupted stays controllable, and the
    # next readiness completes it.
    drop_in.unlink()
    assert f.service.verify_service(directory, receipt) == earlier
    assert f.service.widen_service_stop_budget(directory, 432_000) == ["/usr/bin/systemctl", "--user", "daemon-reload"]
    assert drop_in.read_bytes() == written
    # A forged record is refused like a forged unit budget.
    for forged in ({"gateway_shutdown_horizon_ms": 432_000, "stop_timeout_seconds": 3600},
                   {"gateway_shutdown_horizon_ms": "432000", "stop_timeout_seconds": 482}):
        with pytest.raises(Exception, match="budget is unsupported"):
            f.service.verify_service(directory, {**receipt, "service_stop_budget": forged})


# Under a unit at the earlier 397-second budget and a record for a 432-second
# horizon, a re-widening killed after its record leaves the drop-in an earlier,
# shorter report wrote: a canonical budget above the unit's and below the
# record's. Nothing else in the owned drop-in is accepted.
EARLIER_DROP_IN_BUDGETS = [398, service_budget(392_000), service_budget(432_000) - 1]
FOREIGN_DROP_INS = [b"[Service]\nTimeoutStopSec=397\n", b"[Service]\nTimeoutStopSec=45\n",
                    f"[Service]\nTimeoutStopSec={service_budget(432_000) + 1}\n".encode(),
                    b"[Service]\nTimeoutStopSec=0442\n", b"[Service]\nTimeoutStopSec=442\nKillMode=none\n",
                    b"[Service]\nTimeoutStopSec=442", b"[Unit]\nTimeoutStopSec=442\n"]


@pytest.mark.parametrize("background", ["linux"], indirect=True)
async def test_an_earlier_owned_drop_in_below_its_record_stays_controllable(background):
    f = background
    directory = await f.prepared()
    earlier, _ = earlier_unit(f, directory)
    drop_in = Path(earlier["unit_path"] + ".d") / DROP_IN
    f.service.widen_service_stop_budget(directory, 432_000)
    receipt = f.deployment.read_receipt(directory)
    for budget in EARLIER_DROP_IN_BUDGETS:
        drop_in.write_bytes(f"[Service]\nTimeoutStopSec={budget}\n".encode())
        assert f.service.verify_service(directory, receipt) == earlier
    for content in FOREIGN_DROP_INS:
        drop_in.write_bytes(content)
        with pytest.raises(Exception, match="definition changed"):
            f.service.verify_service(directory, receipt)


@pytest.mark.parametrize("background", ["linux"], indirect=True)
async def test_the_manager_stop_command_outlasts_a_stop_budget_drop_in(background):
    f = background
    directory = await f.prepared()
    earlier, _ = earlier_unit(f, directory)
    f.service.widen_service_stop_budget(directory, 432_000)
    calls = []
    class Manager(f.service.UserServiceManager):
        async def observe(self, service):
            return {"loaded": True, "pid": 123, "state": "running"}
        async def command(self, arguments, **kwargs):
            calls.append((arguments[2:4], kwargs.get("timeout")))
            return 0, ""
    await Manager().stop(earlier)
    assert calls == [(["disable", "--now"], service_budget(432_000) + 15)]


# An application whose gateway lets in-flight turns finish on shutdown, for a
# window of W milliseconds, advertises the plain horizon plus 10 seconds for
# the drain's own stop plus W, and reports that at readiness. Meta caps W at
# 300 seconds. Without a window it advertises the plain horizon.
DRAIN_WINDOWS_MS = [60_000, 300_000]
RELOAD = ["/usr/bin/systemctl", "--user", "daemon-reload"]


def drained_horizon_ms(window_ms):
    """The horizon an application with a drain window of window_ms reports."""
    return gateway_horizon_ms() + (10_000 + window_ms if window_ms else 0)


async def worker_stop_window(monkeypatch, horizon_ms):
    """The application stop window a worker waits after a report of horizon_ms."""
    supervisor = deployment_worker.Supervisor.__new__(deployment_worker.Supervisor)
    supervisor.gateway_shutdown_horizon_ms = horizon_ms
    windows = observed_stop_window(monkeypatch, supervisor)
    await supervisor.terminate()
    return windows


def drop_in_budget(unit_path):
    return int(re.fullmatch(rb"\[Service\]\nTimeoutStopSec=([0-9]+)\n",
                            (Path(unit_path + ".d") / DROP_IN).read_bytes()).group(1))


@pytest.mark.parametrize("background", ["linux"], indirect=True)
async def test_without_a_drain_window_every_local_budget_is_the_plain_horizons(background, monkeypatch):
    # The plain horizon's budgets, exactly: the application's stop window
    # 452 s, the worker's 467 s, the unit's 482 s and the stop command's 497 s.
    f = background
    horizon = drained_horizon_ms(0)
    assert horizon == 432_000
    assert deadlines.shutdown_budget(horizon) == {"application": 452, "force_reap": 10, "worker": 467,
                                                  "service": 482, "command": 497, "legacy_service": 45}
    directory = await f.prepared()
    f.deployment.update_receipt(directory, gateway_shutdown_horizon_ms=horizon)
    service = f.service.prepare_service(directory, f.deployment.read_receipt(directory))
    assert service["stop_timeout_seconds"] == 482
    assert f.stop_timeout(Path(service["unit_path"]).read_bytes()) == 482
    # The report needs no widening: the unit already carries its budget.
    assert f.service.widen_service_stop_budget(directory, horizon) is None
    assert not Path(service["unit_path"] + ".d").exists()
    assert await worker_stop_window(monkeypatch, horizon) == [452]


@pytest.mark.parametrize("background", ["linux"], indirect=True)
@pytest.mark.parametrize("window_ms", DRAIN_WINDOWS_MS, ids=["window-60s", "window-300s"])
async def test_a_drain_windows_report_widens_every_local_budget(background, monkeypatch, window_ms):
    # A fresh installation's unit is sized for the plain horizon before the
    # first report. The report of the drained horizon widens it through the
    # owned drop-in, and the stop command and the worker follow the report.
    f = background
    horizon = drained_horizon_ms(window_ms)
    budget = service_budget(horizon)
    directory = await f.prepared()
    service = f.service.prepare_service(directory, f.deployment.read_receipt(directory))
    assert service["stop_timeout_seconds"] == service_budget(gateway_horizon_ms())
    f.deployment.update_receipt(directory, gateway_shutdown_horizon_ms=horizon)
    assert f.service.widen_service_stop_budget(directory, horizon) == RELOAD
    record = {"gateway_shutdown_horizon_ms": horizon, "stop_timeout_seconds": budget}
    assert f.deployment.read_receipt(directory)["service_stop_budget"] == record
    assert drop_in_budget(service["unit_path"]) == budget
    # A restart clears the reported horizon until the new child reports. The
    # widened drop-in and its record stay, and the unit stays controllable.
    f.deployment.update_receipt(directory, gateway_shutdown_horizon_ms=None)
    receipt = f.deployment.read_receipt(directory)
    assert receipt["service_stop_budget"] == record and drop_in_budget(service["unit_path"]) == budget
    assert f.service.verify_service(directory, receipt) == service
    calls = []
    class Manager(f.service.UserServiceManager):
        async def observe(self, service):
            return {"loaded": True, "pid": 123, "state": "running"}
        async def command(self, arguments, **kwargs):
            calls.append((arguments[2:4], kwargs.get("timeout")))
            return 0, ""
    await Manager().stop(service)
    assert calls == [(["disable", "--now"], budget + 15)]
    assert await worker_stop_window(monkeypatch, horizon) == [horizon // 1000 + TRANSPORT_AND_ADAPTER_MARGIN]


@pytest.mark.parametrize("background", ["linux"], indirect=True)
@pytest.mark.parametrize("window_ms", DRAIN_WINDOWS_MS, ids=["window-60s", "window-300s"])
async def test_a_unit_created_after_a_drain_windows_report_is_sized_for_it(background, window_ms):
    f = background
    horizon = drained_horizon_ms(window_ms)
    directory = await f.prepared()
    f.deployment.update_receipt(directory, gateway_shutdown_horizon_ms=horizon)
    service = f.service.prepare_service(directory, f.deployment.read_receipt(directory))
    assert service["stop_timeout_seconds"] == service_budget(horizon)
    assert f.stop_timeout(Path(service["unit_path"]).read_bytes()) == service_budget(horizon)


@pytest.mark.parametrize("background", ["linux"], indirect=True)
async def test_a_grown_drain_window_widens_again_and_a_smaller_report_never_narrows(background):
    f = background
    first, grown = (drained_horizon_ms(window) for window in DRAIN_WINDOWS_MS)
    directory = await f.prepared()
    service = f.service.prepare_service(directory, f.deployment.read_receipt(directory))
    f.service.widen_service_stop_budget(directory, first)
    # An update raises the window. Until the new child reports, its start
    # keeps the budget the smaller window's report widened.
    f.deployment.update_receipt(directory, gateway_shutdown_horizon_ms=None)
    assert drop_in_budget(service["unit_path"]) == service_budget(first)
    assert f.service.widen_service_stop_budget(directory, grown) == RELOAD
    record = {"gateway_shutdown_horizon_ms": grown, "stop_timeout_seconds": service_budget(grown)}
    assert f.deployment.read_receipt(directory)["service_stop_budget"] == record
    assert drop_in_budget(service["unit_path"]) == service_budget(grown)
    # A later, smaller report (the window lowered, removed, or unreported)
    # never narrows it.
    for later in (first, gateway_horizon_ms(), None):
        f.service.widen_service_stop_budget(directory, later)
        assert f.deployment.read_receipt(directory)["service_stop_budget"] == record
        assert drop_in_budget(service["unit_path"]) == service_budget(grown)
