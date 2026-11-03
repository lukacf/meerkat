"""Ephemeral browser access to exact, freshly observed owned remote interfaces."""
from __future__ import annotations

import asyncio
from copy import deepcopy
import json
import re
import time
import uuid
from urllib.parse import quote

from .deployment import await_validation, now, package_console_scope, package_configuration_fields
from .projects import DraftConflict
from .application_access import ENTRY, selected_application_entry, application_available

HASH = re.compile(r"[a-f0-9]{64}")
CONSOLE = re.compile(r"http://127\.0\.0\.1:([0-9]{4,5})/console")
UNIT = re.compile(r"mobkit-launch\.([a-f0-9-]{36})\.service")
KEYS = ("guide_id", "guide_version", "build_id", "deployment_id", "accepted_source_digest",
        "package_sha256", "transferred_source_digest", "controller_sha256")


def console_port(value):
    match = CONSOLE.fullmatch(value) if isinstance(value, str) else None
    if (not match or not 1024 <= int(match[1]) <= 65535
            or value != f"http://127.0.0.1:{int(match[1])}/console"):
        return None
    return int(match[1])


def retained_product_status(receipt):
    """Read only an original source-joined product factory observation.

    A retained control worker is distinct from the application and a legacy
    system service. Saved facts never create or renew either owner's lease.
    """
    if receipt.get("product_runtime") is not True:
        return None
    observation = receipt.get("observation")
    if (not isinstance(observation, dict) or observation.get("available") is not True
            or receipt.get("remote_admission_requested") is not True or receipt.get("admitted") is not True
            or receipt.get("worker_live") is not True or receipt.get("cleanup_confirmed") is not False
            or receipt.get("service") != {} or receipt.get("service_owned") is not False
            or receipt.get("service_observed") is not False or receipt.get("source_current") is not True):
        return None
    from .remote_deployment import product_control_observation, build_guide_version
    try:
        product = product_control_observation(receipt, {"scope": "retained-original-product-owner",
            "product_control": receipt.get("product_control")})
        if product is None:
            return None
        actual = product["factory_status"]
        if (actual.get("source_verified") is not True or not isinstance(actual.get("status"), str)
                or actual.get("status") != receipt.get("status") or type(actual.get("guide_version")) is not int
                or actual.get("guide_version") != build_guide_version(receipt)
                or actual.get("target") != "local"):
            return None
        request = product.get("action_request")
        if request is not None and (not isinstance(request, dict)
                or request.get("phase") not in {"pending", "settled", "unknown"}
                or request.get("guide_id") != receipt["guide_id"]
                or request.get("deployment_id") != receipt["deployment_id"]):
            return None
        if actual.get("phase") is not None and not isinstance(actual["phase"], str):
            return None
        update = actual.get("update")
        if update is not None and (not isinstance(update, dict)
                or update.get("status") is not None and not isinstance(update["status"], str)):
            return None
        return product
    except (DraftConflict, KeyError, TypeError, ValueError):
        return None


def access_binding(receipt):
    service = receipt.get("service") if isinstance(receipt.get("service"), dict) else {}
    value = {**{key: receipt.get(key) for key in KEYS}, "endpoint": deepcopy(receipt.get("endpoint")),
        "host": deepcopy(receipt.get("host")), "destination": receipt.get("destination"),
        "host_key_fingerprint": receipt.get("host_key_fingerprint"),
        "service_name": service.get("service"), "service_source_digest": service.get("source_digest"),
        "console_url": service.get("console_url")}
    product = retained_product_status(receipt)
    if product is not None:
        actual = product["factory_status"]
        # A restart must revoke the old forward even when the port is reused.
        value.update(console_url=actual.get("console_url"),
            product_control_binding=deepcopy(product["binding"]),
            product_runtime_binding={key: deepcopy(actual.get(key)) for key in (
                "runtime_pid", "startup_attempt_id", "interactive_lifecycle_id", "execution_mode",
                "service_instance_id", "service_supervisor_pid")})
    return value


def native_ready(receipt, current):
    if receipt.get("product_runtime") is True:
        product = retained_product_status(receipt)
        if (current is not True or product is None or receipt.get("status") != "running"
                or receipt.get("ready") is not True or receipt.get("pending_operation") is not None
                or (product.get("action_request") or {}).get("phase") in {"pending", "unknown"}):
            return False
        actual = product["factory_status"]
        from .installed_feedback import _mutating
        if _mutating(actual):
            return False
        from .remote_deployment import operation_identity
        if not operation_identity(actual.get("startup_attempt_id")):
            return False
        if actual.get("execution_mode") == "interactive":
            if not operation_identity(actual.get("interactive_lifecycle_id")):
                return False
        elif actual.get("execution_mode") == "background":
            if (not operation_identity(actual.get("service_instance_id"))
                    or type(actual.get("service_supervisor_pid")) is not int or actual["service_supervisor_pid"] <= 0):
                return False
        else:
            return False
        bootstrap = actual.get("bootstrap")
        if not isinstance(bootstrap, dict) or not isinstance(bootstrap.get("counts"), dict):
            return False
        from .deployment_evidence import project_deployment_evidence
        native = project_deployment_evidence({"id": receipt["guide_id"]}, actual)
        observed = product.get("native_evidence")
        return (isinstance(observed, dict) and observed.get("owned_source_verified") is True
            and observed.get("native_runtime_available") is True
            and observed.get("deployment_id") == receipt["deployment_id"]
            and observed.get("build_id") == receipt["build_id"]
            and observed.get("source_digest") == receipt["accepted_source_digest"]
            and native["native_runtime_available"] is True
            and type(actual.get("runtime_pid")) is int and actual["runtime_pid"] > 0
            and console_port(actual.get("console_url")) is not None)
    service = receipt.get("service") if isinstance(receipt.get("service"), dict) else {}
    observation = receipt.get("observation") if isinstance(receipt.get("observation"), dict) else {}
    name = service.get("service")
    unit = UNIT.fullmatch(name) if isinstance(name, str) else None
    try:
        unit_owned = unit is not None and str(uuid.UUID(unit[1])) == unit[1]
    except (ValueError, KeyError):
        unit_owned = False
    return (current is True and receipt.get("status") == "running"
        and receipt.get("remote_admission_requested") is True and receipt.get("admitted") is True
        and receipt.get("worker_live") is False and not receipt.get("pending_operation")
        and observation.get("available") is True
        and receipt.get("service_owned") is True and receipt.get("service_observed") is True
        and receipt.get("ready") is True and receipt.get("source_current") is True
        and service.get("ready") is True and service.get("status") == "running"
        and service.get("definition_verified") is True and service.get("source_current") is True
        and service.get("scope") == "native-bootstrap-and-console" and unit_owned
        and isinstance(service.get("source_digest"), str)
        and HASH.fullmatch(service["source_digest"]) is not None
        and service["source_digest"] == receipt.get("transferred_source_digest")
        and console_port(service.get("console_url")) is not None)


async def browser_responds(port, authentication):
    """Bounded credential-free GET, with no proxy, cookies, redirects or response storage."""
    writer = None
    try:
        async with asyncio.timeout(4):
            reader, writer = await asyncio.open_connection("127.0.0.1", port, limit=8192)
            writer.write((f"GET /console HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n"
                "Connection: close\r\nAccept: text/html\r\n\r\n").encode("ascii"))
            await writer.drain()
            headers = await reader.readuntil(b"\r\n\r\n")
            if len(headers) > 8192:
                return False
            lines = headers.decode("iso-8859-1").split("\r\n")
            status = lines[0].split(" ")
            if len(status) < 2 or status[0] not in {"HTTP/1.0", "HTTP/1.1"}:
                return False
            locations = [line.partition(":")[2].strip() for line in lines[1:]
                         if line.partition(":")[0].lower() == "location"]
            if authentication == "local-password":
                return status[1] == "303" and locations == ["/login"]
            return authentication == "loopback-only" and status[1] == "200" and not locations
    except (OSError, TimeoutError, ValueError, asyncio.IncompleteReadError, asyncio.LimitOverrunError):
        return False
    finally:
        if writer:
            writer.close()
            try:
                await asyncio.wait_for(writer.wait_closed(), timeout=1)
            except (OSError, TimeoutError):
                pass


async def application_responds(port, path):
    """Observe one bounded HTTP status, without retaining headers or bodies."""
    writer = None
    try:
        async with asyncio.timeout(4):
            reader, writer = await asyncio.open_connection("127.0.0.1", port, limit=8192)
            target = quote(path, safe="/%:@!$&'()*+,;=-._~")
            writer.write((f"GET {target} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n"
                "Connection: close\r\nAccept: text/html\r\n\r\n").encode("ascii"))
            await writer.drain()
            headers = await reader.readuntil(b"\r\n\r\n")
            if len(headers) > 8192:
                return None
            status = headers.partition(b"\r\n")[0]
            match = re.fullmatch(rb"HTTP/1\.[01] ([2-5][0-9]{2})(?: [^\r\n]*)?", status)
            return int(match[1]) if match else None
    except (OSError, TimeoutError, ValueError, asyncio.IncompleteReadError, asyncio.LimitOverrunError):
        return None
    finally:
        if writer:
            writer.close()
            try:
                await asyncio.wait_for(writer.wait_closed(), timeout=1)
            except (OSError, TimeoutError):
                pass


def entry_binding(receipt, entry_id):
    value = access_binding(receipt)
    if entry_id == "native-console":
        return value
    entry = selected_application_entry(receipt, entry_id)
    if entry is None:
        return None
    return {**value, "entry_id": entry_id, "metadata_sha256": entry["metadata_sha256"],
        "entry_path": entry["path"], "destination_port": entry["destination_port"],
        "listener_token": entry["listener_token"]}


def entry_ready(receipt, current, entry_id):
    if not native_ready(receipt, current):
        return False
    if entry_id == "native-console":
        return True
    # Product and ordinary entries both require their source-joined listener
    # token. Native availability alone cannot attest an authored interface.
    entry = selected_application_entry(receipt, entry_id)
    return bool(entry and entry.get("listener_owned") is True
        and type(entry.get("destination_port")) is int and 1024 <= entry["destination_port"] <= 65535
        and isinstance(entry.get("listener_token"), str) and HASH.fullmatch(entry["listener_token"]))


class RemoteBrowserAccessService:
    """Explicit entry access with one exact owned forward per destination port."""

    def __init__(self, remote, *, transport=None, browser_check=browser_responds,
                 application_check=application_responds, monitor_interval=5):
        self.remote, self.transport = remote, transport
        self.browser_check, self.application_check = browser_check, application_check
        self.monitor_interval = monitor_interval
        self.records, self.locks, self.monitors = {}, {}, {}
        self.shutting_down = False

    def key(self, guide_id, deployment_id, entry_id="native-console"):
        for value in (guide_id, deployment_id):
            try:
                if not isinstance(value, str) or str(uuid.UUID(value)) != value:
                    raise ValueError
            except (ValueError, AttributeError):
                raise ValueError("Choose an exact owned remote installation.") from None
        if entry_id != "native-console" and (not isinstance(entry_id, str) or not ENTRY.fullmatch(entry_id)):
            raise ValueError("Choose an entry declared by the installed application.")
        return (guide_id, deployment_id) if entry_id == "native-console" else (guide_id, deployment_id, entry_id)

    def lock(self, key):
        return self.locks.setdefault(key[:2], asyncio.Lock())

    def deployment_keys(self, key):
        return [candidate for candidate in self.records if candidate[:2] == key[:2]]

    def public(self, key):
        record = self.records.get(key)
        if not record:
            return {"protocol": 1, "guide_id": key[0], "deployment_id": key[1], "access_id": None,
                "entry_id": key[2] if len(key) == 3 else "native-console", "status": "closed",
                "native_ready": False, "cleanup_confirmed": True, "local_url": None,
                "observed_at": now(), "detail": "Connect to open this installed interface on this computer."}
        result = deepcopy(record["public"])
        result["cleanup_confirmed"] = record["revoked"] is True and record.get("handle") is None
        if result["status"] == "ready" and time.monotonic() - record["verified_at"] > 15:
            result.update(status="unavailable", local_url=None, native_ready=False,
                detail="Browser access needs a fresh check. The remote application may still be running.")
            if result["entry_id"] != "native-console":
                result["transport_reachable"] = False
        return result

    async def _scope(self, receipt, entry_id):
        if entry_id != "native-console":
            return {"health_scope": "declared-application-transport", "custom_interfaces_verified": False}
        # Reading, hashing and unpacking the saved package must not hold the
        # event loop.
        files, _, _ = await await_validation(self.remote.frozen,
            self.remote.directory(receipt["guide_id"], receipt["deployment_id"]), receipt)
        fields = package_configuration_fields(json.loads(files["deployment.json"][0]), files)
        return package_console_scope(files, fields, await self.remote.guide.get(receipt["guide_id"]))

    async def _closed_public(self, receipt, entry_id="native-console", *, scope=None):
        scope = scope if scope is not None else await self._scope(receipt, entry_id)
        binding = access_binding(receipt)
        value = {"protocol": 1, **{name: receipt[name] for name in KEYS}, "access_id": None,
            "current_guide_version": receipt.get("current_guide_version"),
            "entry_id": entry_id, "status": "closed", "native_ready": False,
            "cleanup_confirmed": True, "local_url": None, "observed_at": now(), "scope": scope,
            "service_name": binding["service_name"], "service_source_digest": binding["service_source_digest"],
            "console_url": binding["console_url"],
            "detail": "Connect to open this installed interface on this computer."}
        for name in ("product_control_binding", "product_runtime_binding"):
            if name in binding:
                value[name] = deepcopy(binding[name])
        if entry_id != "native-console":
            binding = entry_binding(receipt, entry_id) or {}
            value.update({name: binding.get(name) for name in (
                "metadata_sha256", "entry_path", "destination_port", "listener_token")})
            value.update(transport_reachable=False, http_status=None, application_verified=False,
                authentication_verified=False, workflow_verified=False)
        return value

    async def _close_locked(self, key, *, status="closed", detail=None, code=None):
        record = self.records.get(key)
        if not record:
            return self.public(key)
        handle = record.get("handle")
        aliases = [self.records[item] for item in self.deployment_keys(key)
                   if handle is not None and self.records[item].get("handle") is handle] if handle else [record]
        # Withdraw every alias before awaiting cleanup of the shared listener.
        for alias in aliases:
            alias.update(revoked=True, closing_status=status, closing_detail=detail)
            alias["public"].update(status=status, local_url=None, native_ready=False, observed_at=now(),
                detail=detail or "Browser access is closed. Your remote application continues running.")
            if alias["public"]["entry_id"] != "native-console":
                alias["public"]["transport_reachable"] = False
            if code:
                alias["public"]["code"] = code
            else:
                alias["public"].pop("code", None)
        if handle:
            try:
                cleaned = await handle.close()
            except (OSError, RuntimeError, TimeoutError):
                cleaned = False
            for alias in aliases:
                if cleaned is True:
                    alias["handle"] = None
                else:
                    alias["public"].update(status="cleanup-needed", code="cleanup-unavailable",
                        detail="The local browser connection has not confirmed cleanup. Recheck or close it again before reconnecting.")
        return self.public(key)

    def _withdraw_deployment(self, key, status, detail, *, candidates=None):
        for candidate in candidates if candidates is not None else self.deployment_keys(key):
            record = self.records[candidate]
            record.update(revoked=True, closing_status=status, closing_detail=detail)
            record["public"].update(status=status, local_url=None, native_ready=False,
                observed_at=now(), detail=detail)
            if record["public"]["entry_id"] != "native-console":
                record["public"]["transport_reachable"] = False

    async def _revoke_locked(self, key, *, status="unavailable", detail=None, custom_only=False):
        detail = detail or "Browser access could not be freshly verified. Your remote application may still be running."
        candidates = self.deployment_keys(key)
        if custom_only:
            handles = {id(self.records[item]["handle"]) for item in candidates
                       if len(item) == 3 and self.records[item].get("handle")}
            candidates = [item for item in candidates if len(item) == 3 or (
                self.records[item].get("handle") and id(self.records[item]["handle"]) in handles)]
        self._withdraw_deployment(key, status, detail, candidates=candidates)
        handled = set()
        for candidate in candidates:
            handle = self.records[candidate].get("handle")
            if handle is not None and id(handle) in handled:
                continue
            if handle is not None:
                handled.add(id(handle))
            await self._close_locked(candidate, status=status, detail=detail)
        return self.public(key)

    async def revoke(self, guide_id, deployment_id, detail):
        key = self.key(guide_id, deployment_id)
        async with self.lock(key):
            return await self._revoke_locked(key, status="closed", detail=detail)

    async def reconcile(self, receipt, current):
        key = self.key(receipt["guide_id"], receipt["deployment_id"])
        async with self.lock(key):
            if not native_ready(receipt, current) or any(
                    self.records[candidate].get("handle") and self.records[candidate]["base_binding"] != access_binding(receipt)
                    for candidate in self.deployment_keys(key)):
                await self._revoke_locked(key)
                return
            if not application_available(receipt):
                await self._revoke_locked(key, custom_only=True)
            for candidate in self.deployment_keys(key):
                record = self.records[candidate]
                entry_id = record["public"]["entry_id"]
                if record.get("handle") and (not entry_ready(receipt, current, entry_id)
                        or record["binding"] != entry_binding(receipt, entry_id)):
                    await self._close_locked(candidate, status="unavailable",
                        detail="Browser access was withdrawn because the installed source or runtime could not be freshly verified. The remote application may still be running.")

    async def _context(self, guide_id, deployment_id, directory, *, force):
        from .remote_deployment import read_receipt
        receipt = read_receipt(directory)
        if await self.remote.cloud_status(receipt) is None and (force or time.monotonic() - self.remote.last_observed.get(deployment_id, -100) >= 3):
            await self.remote._probe(directory, "status")
        receipt = read_receipt(directory)
        state = await self.remote.current_state(receipt)
        current = state is not None
        receipt["current_guide_version"] = state["version"] if current else None
        return receipt, bool(current and await self.remote.cloud_status(receipt) is None
            and self.remote.directory(guide_id) == directory)

    async def open(self, guide_id, version, build_id, deployment_id, entry_id="native-console"):
        from .ssh_browser_access import BrowserAccessError, SshBrowserAccessTransport
        key = self.key(guide_id, deployment_id, entry_id)
        async with self.remote.lock:
            try:
                directory, receipt = await await_validation(self.remote.stored, guide_id, deployment_id)
            except (ValueError, LookupError, OSError, RuntimeError, TypeError, KeyError):
                async with self.lock(key):
                    await self._revoke_locked(key)
                raise DraftConflict("The saved installation needs a fresh ownership check before connecting.") from None
            async with self.remote.operation_lock(directory):
                async with self.lock(key):
                    if self.shutting_down:
                        raise DraftConflict("The wizard is closing its local browser connections.")
                    if self.remote.directory(guide_id) != directory:
                        await self._revoke_locked(key)
                        raise DraftConflict("Choose the selected current installed application before connecting.")
                    await self.remote.require_current_request(guide_id, version)
                    if receipt["build_id"] != build_id:
                        raise DraftConflict("Choose the current accepted installed application before connecting.")
                    try:
                        receipt, current = await self._context(guide_id, deployment_id, directory, force=True)
                        if receipt.get("current_guide_version") != version:
                            raise DraftConflict("The guide changed while checking this browser connection. Reload before connecting.")
                    except (ValueError, LookupError, OSError, RuntimeError, TypeError, KeyError):
                        await self._revoke_locked(key)
                        raise DraftConflict("The installation needs a fresh source and runtime check before connecting.") from None
                    if not entry_ready(receipt, current, entry_id):
                        if not native_ready(receipt, current):
                            await self._revoke_locked(key)
                        elif entry_id != "native-console" and not application_available(receipt):
                            await self._revoke_locked(key, custom_only=True)
                        else:
                            await self._close_locked(key, status="unavailable")
                        raise DraftConflict("Recheck the current accepted running application and ownership of this declared interface before connecting.")
                    binding = entry_binding(receipt, entry_id)
                    try:
                        scope = await self._scope(receipt, entry_id)
                        await self.remote.require_current_request(guide_id, version)
                    except (ValueError, LookupError, OSError, RuntimeError, TypeError, KeyError):
                        await self._revoke_locked(key)
                        raise DraftConflict("The installed source needs a fresh ownership check before connecting.") from None
                    record = self.records.get(key)
                    if record and record.get("handle"):
                        if record["binding"] == binding and record["public"]["status"] == "ready":
                            try:
                                await self._verify_locked(key, receipt, current)
                                await self.remote.require_current_request(guide_id, version)
                            except (ValueError, LookupError, OSError, RuntimeError, TypeError, KeyError):
                                await self._revoke_locked(key)
                            self._start_monitor(key)
                            return self.public(key)
                        await self._close_locked(key)
                        if record.get("handle"):
                            return self.public(key)
                    port = console_port(binding["console_url"]) if entry_id == "native-console" else binding["destination_port"]
                    record = {"binding": binding, "base_binding": access_binding(receipt), "port": port,
                        "handle": None, "verified_at": 0, "revoked": False,
                        "public": await self._closed_public(receipt, entry_id, scope=scope)}
                    record["public"].update(access_id=str(uuid.uuid4()), status="connecting", cleanup_confirmed=False,
                        detail="Connecting this installed interface to your computer.")
                    self.records[key] = record
                    # Pages on the same port share custody. An uncertain old handle
                    # cannot be bypassed by choosing a different entry identifier.
                    for candidate in self.deployment_keys(key):
                        alias = self.records[candidate]
                        if candidate == key or not alias.get("handle") or alias["port"] != port:
                            continue
                        if alias["revoked"] or alias["base_binding"] != record["base_binding"]:
                            record["handle"] = alias["handle"]
                            await self._close_locked(key, status="unavailable")
                            if record.get("handle"):
                                self._start_monitor(key)
                                return self.public(key)
                        else:
                            record["handle"] = alias["handle"]
                        break
                    try:
                        await self.remote.require_current_request(guide_id, version)
                        if record["handle"] is None:
                            transport = self.transport
                            if transport is None:
                                select = getattr(self.remote, "browser_transport", None)
                                transport = select(receipt) if select is not None else None
                                transport = transport or SshBrowserAccessTransport()
                            record["handle"] = await transport.open(receipt["endpoint"],
                                expected_fingerprint=receipt["host_key_fingerprint"], port=port)
                            record["revoked"] = False
                        receipt, current = await self._context(guide_id, deployment_id, directory, force=True)
                        if not await self._verify_locked(key, receipt, current):
                            return self.public(key)
                        await self.remote.require_current_request(guide_id, version)
                    except BrowserAccessError as error:
                        if error.code == "cleanup-unavailable" and getattr(error, "handle", None) is not None:
                            record.update(handle=error.handle, revoked=True, closing_status="unavailable",
                                closing_detail="Browser startup cleanup still needs confirmation.")
                            record["public"].update(status="cleanup-needed", code=error.code,
                                detail="The local browser connection has not confirmed cleanup. Recheck or close it again before reconnecting.",
                                local_url=None, native_ready=False, observed_at=now())
                            self._start_monitor(key)
                            return self.public(key)
                        await self._close_locked(key, status="blocked" if error.code == "port-unavailable" else "unavailable",
                            detail=str(error), code=error.code)
                        return self.public(key)
                    except asyncio.CancelledError:
                        await asyncio.shield(self._close_locked(key, status="unavailable"))
                        raise
                    except (ValueError, LookupError, OSError, RuntimeError, TypeError, KeyError):
                        await self._revoke_locked(key, status="unavailable",
                            detail="Browser access could not be verified. Your remote application may still be running.")
                        return self.public(key)
                    self._start_monitor(key)
                    return self.public(key)

    async def _verify_locked(self, key, receipt, current):
        record = self.records[key]
        if record["revoked"]:
            await self._close_locked(key, status=record.get("closing_status", "closed"), detail=record.get("closing_detail"))
            return False
        entry_id = record["public"]["entry_id"]
        if (not native_ready(receipt, current) or record["base_binding"] != access_binding(receipt)
                or self.remote.directory(key[0]).name != key[1]):
            await self._revoke_locked(key)
            return False
        if entry_id != "native-console" and not application_available(receipt):
            await self._revoke_locked(key, custom_only=True)
            return False
        qualified = entry_ready(receipt, current, entry_id) and record["binding"] == entry_binding(receipt, entry_id)
        handle = record.get("handle")
        http_status = None
        if qualified and handle and await handle.observe():
            if entry_id == "native-console":
                qualified = await self.browser_check(record["port"], record["public"]["scope"]["authentication"])
                if receipt.get("product_runtime") is True:
                    # The actual runtime can change during the credential-free
                    # HTTP check. Rejoin its original actor before publishing.
                    receipt, current = await self._context(key[0], key[1], self.remote.directory(*key[:2]), force=True)
                    qualified = qualified and entry_ready(receipt, current, entry_id) and record["binding"] == entry_binding(receipt, entry_id)
            else:
                http_status = await self.application_check(record["port"], record["binding"]["entry_path"])
                qualified = type(http_status) is int and (200 <= http_status < 400 or http_status in {401, 403})
                # Listener custody can change while HTTP is in flight. Rejoin a
                # new bounded destination observation before publishing the URL.
                receipt, current = await self._context(key[0], key[1], self.remote.directory(*key[:2]), force=True)
                qualified = qualified and entry_ready(receipt, current, entry_id) and record["binding"] == entry_binding(receipt, entry_id)
            qualified = qualified and await handle.observe()
        else:
            qualified = False
        cloud_status = await self.remote.cloud_status(receipt)
        state = await self.remote.current_state(receipt)
        if (not native_ready(receipt, current) or record["base_binding"] != access_binding(receipt)
                or state is None or cloud_status is not None
                or self.remote.directory(key[0]).name != key[1]):
            await self._revoke_locked(key)
            return False
        if entry_id != "native-console" and not application_available(receipt):
            await self._revoke_locked(key, custom_only=True)
            return False
        if not qualified:
            await self._close_locked(key, status="unavailable",
                detail="Browser access could not be freshly verified. Your remote application may still be running.")
            if entry_id != "native-console" and type(http_status) is int:
                record["public"]["http_status"] = http_status
            return False
        record["verified_at"] = time.monotonic()
        url = record["binding"]["console_url"] if entry_id == "native-console" else (
            f'http://127.0.0.1:{record["port"]}' + quote(record["binding"]["entry_path"], safe="/%:@!$&'()*+,;=-._~"))
        record["public"].update(status="ready", local_url=url, native_ready=True,
            current_guide_version=state["version"], observed_at=now(),
            detail="Your installed native Console is ready to open on this computer." if entry_id == "native-console" else
                "This installed interface is responding. Sign-in and useful tasks still need to be checked in the app.")
        if entry_id != "native-console":
            record["public"].update(transport_reachable=True, http_status=http_status)
        record["public"].pop("code", None)
        return True

    async def status(self, guide_id, deployment_id, entry_id="native-console"):
        key = self.key(guide_id, deployment_id, entry_id)
        if key not in self.records or not self.records[key].get("handle"):
            try:
                _, receipt = await await_validation(self.remote.stored, guide_id, deployment_id)
                state = await self.remote.current_state(receipt)
                receipt["current_guide_version"] = state["version"] if state is not None else None
            except (ValueError, LookupError, OSError, RuntimeError, TypeError, KeyError):
                async with self.lock(key):
                    await self._revoke_locked(key)
                raise DraftConflict("The saved installation needs a fresh ownership check.") from None
            record = self.records.get(key)
            if not record or record["binding"] != entry_binding(receipt, entry_id):
                return await self._closed_public(receipt, entry_id)
            record["public"]["current_guide_version"] = receipt["current_guide_version"]
            return self.public(key)
        try:
            directory, _ = await await_validation(self.remote.stored, guide_id, deployment_id)
        except (ValueError, LookupError, OSError, TypeError, KeyError):
            async with self.lock(key):
                return await self._revoke_locked(key, status="unavailable",
                    detail="Browser access needs a fresh ownership check. Your remote application may still be running.")
        async with self.remote.operation_lock(directory):
            async with self.lock(key):
                try:
                    record = self.records[key]
                    if record["revoked"]:
                        return await self._close_locked(key, status=record.get("closing_status", "closed"), detail=record.get("closing_detail"))
                    receipt, current = await self._context(guide_id, deployment_id, directory, force=False)
                    await self._verify_locked(key, receipt, current)
                except (ValueError, LookupError, OSError, RuntimeError, TypeError, KeyError):
                    await self._revoke_locked(key, status="unavailable",
                        detail="Browser access needs a fresh ownership check. Your remote application may still be running.")
                return self.public(key)

    async def close(self, guide_id, deployment_id, access_id):
        key = self.key(guide_id, deployment_id)
        async with self.lock(key):
            matches = [candidate for candidate in self.deployment_keys(key)
                       if self.records[candidate]["public"]["access_id"] == access_id]
            if len(matches) != 1:
                raise DraftConflict("Close only the exact owned browser connection shown in this setup.")
            return await self._close_locked(matches[0])

    def _start_monitor(self, key):
        group = key[:2]
        task = self.monitors.get(group)
        if task and not task.done():
            return
        async def watch():
            try:
                while not self.shutting_down and any(self.records[item].get("handle") for item in self.deployment_keys(group)):
                    await asyncio.sleep(self.monitor_interval)
                    for candidate in self.deployment_keys(group):
                        if self.records[candidate].get("handle"):
                            await self.status(*candidate)
            except (ValueError, LookupError, OSError, RuntimeError, TypeError, KeyError):
                async with self.lock(group):
                    await self._revoke_locked(group)
        task = asyncio.create_task(watch())
        self.monitors[group] = task
        def finished(done):
            if self.monitors.get(group) is done:
                self.monitors.pop(group, None)
            if not done.cancelled():
                done.exception()
        task.add_done_callback(finished)

    async def shutdown(self):
        self.shutting_down = True
        for key in list(self.records):
            self._withdraw_deployment(key, "closed", "Browser access is closed. Your remote application continues running.")
        tasks = list(self.monitors.values())
        for task in tasks:
            task.cancel()
        await asyncio.gather(*tasks, return_exceptions=True)
        for key in {candidate[:2] for candidate in self.records}:
            async with self.lock(key):
                await self._revoke_locked(key, status="closed",
                    detail="Browser access is closed. Your remote application continues running.")
