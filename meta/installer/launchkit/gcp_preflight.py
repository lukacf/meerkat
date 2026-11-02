"""Package-bound, read-only Google Cloud preparation for the guided installer.

This service never logs in, changes CLI configuration, enables APIs, provisions,
SSHes, or runs downloaded application code. Checks describe a point-in-time
planning prerequisite, not authorization or a verified running deployment.
"""
from __future__ import annotations

import asyncio
from dataclasses import dataclass
from datetime import datetime, timezone
import hashlib
import json
import math
import os
from pathlib import Path
import re
import shlex
import shutil
import signal
import time
from urllib import parse

import httpx

from .artifact_binding import package_matches, proposal_hash
from .blueprint import Blueprint
from .deployment import await_validation, package_files, require_accepted
from .projects import DraftConflict

COMMAND_TIMEOUT = 15
PROBE_TIMEOUT = 180
MAX_OUTPUT = 256 * 1024
MAX_ERROR = 16 * 1024
MAX_TOKEN = 16 * 1024
MAX_PROJECTS = 100
MAX_ACCOUNTS = 100
SAFE_PATH = "/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin:/opt/homebrew/bin:/opt/homebrew/share/google-cloud-sdk/bin"
ACCOUNT = re.compile(r"[A-Za-z0-9][A-Za-z0-9._+%-]{0,190}@[A-Za-z0-9][A-Za-z0-9.-]{0,190}\Z")
PROJECT = re.compile(r"[a-z][a-z0-9-]{4,28}[a-z0-9]\Z")
SHA = re.compile(r"[a-f0-9]{64}\Z")
CREDENTIAL_ENVIRONMENT = (
    "CLOUDSDK_AUTH_ACCESS_TOKEN", "CLOUDSDK_AUTH_ACCESS_TOKEN_FILE",
    "CLOUDSDK_AUTH_CREDENTIAL_FILE_OVERRIDE", "CLOUDSDK_AUTH_IMPERSONATE_SERVICE_ACCOUNT",
    "GOOGLE_APPLICATION_CREDENTIALS",
)
SERVICES = ("compute.googleapis.com", "iap.googleapis.com")
PERMISSIONS = (
    "compute.instances.create", "compute.disks.create", "compute.instances.setLabels",
    "compute.instances.setTags", "compute.firewalls.create", "compute.firewalls.get", "compute.networks.updatePolicy",
    "compute.subnetworks.use",
    "compute.subnetworks.useExternalIp", "compute.instances.get", "compute.instances.list",
    "compute.projects.get", "compute.globalOperations.get", "compute.zoneOperations.get",
    "compute.instances.setMetadata", "compute.projects.setCommonInstanceMetadata",
    "serviceusage.services.enable", "serviceusage.operations.get",
)
IAP_PERMISSION = "iap.tunnelInstances.accessViaIAP"


# The destination helper Forge 0.9.4 rendered into the packages it accepted.
# A destination deployed from such a package keeps that helper and its stop
# budgets until the package is rebuilt; no other form is accepted.
RETAINED_DESTINATION_HELPER_SHA256 = frozenset({
    "6c92410bf651c415cd903f03b2c99aec16f799e369f0b37f139a1676d3d9a216",
})


class ProbeUnavailable(RuntimeError):
    """A bounded observation could not prove its result; private errors stay local."""


@dataclass(frozen=True)
class CommandResult:
    code: int
    output: str


@dataclass(frozen=True)
class RestResult:
    status: int
    value: object


def _environment():
    values = {key: os.environ[key] for key in ("HOME", "USER", "LOGNAME") if key in os.environ}
    values.update(PATH=SAFE_PATH, LANG="C", LC_ALL="C", CLOUDSDK_CORE_DISABLE_PROMPTS="1")
    # A configured CLI directory can contain the user's existing account store.
    # Credential overrides and endpoint overrides are deliberately not inherited.
    directory = os.environ.get("CLOUDSDK_CONFIG")
    if directory and Path(directory).is_absolute() and len(directory) <= 2000 and not any(ord(c) < 32 for c in directory):
        values["CLOUDSDK_CONFIG"] = directory
    return values


class GcloudReadOnly:
    """Bounded argv-only CLI queries, owning each subprocess through cleanup."""
    def __init__(self, executable=None, *, timeout=COMMAND_TIMEOUT, output_limit=MAX_OUTPUT):
        found = executable or shutil.which("gcloud") or shutil.which("gcloud", path=SAFE_PATH)
        if not found:
            common = Path.home() / "google-cloud-sdk/bin/gcloud"
            found = str(common) if common.is_file() and os.access(common, os.X_OK) else None
        self.executable = str(Path(found).absolute()) if found else None
        self.timeout, self.output_limit = timeout, output_limit
        self.environment = _environment()
        self._pending_groups = set()
        self._pending_processes = {}
        self._unresolved_spawn = False

    @property
    def available(self):
        return bool(self.executable)

    @property
    def cleanup_pending(self):
        for group in tuple(self._pending_groups):
            try:
                os.killpg(group, 0)
            except ProcessLookupError:
                owned = self._pending_processes.get(group)
                if owned is None or (owned[0].returncode is not None and owned[1].done()):
                    self._pending_groups.discard(group)
                    self._pending_processes.pop(group, None)
            except PermissionError:
                pass
        return bool(self._pending_groups) or self._unresolved_spawn

    async def command(self, arguments, *, secret=False):
        if not self.available:
            raise ProbeUnavailable("Google Cloud CLI is not installed.")
        process = None
        spawning = asyncio.create_task(asyncio.create_subprocess_exec(
            self.executable, *arguments, env=self.environment, stdin=asyncio.subprocess.DEVNULL,
            stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.PIPE, start_new_session=True))
        readers = []
        try:
            cancelled = False
            while True:
                try:
                    process = await asyncio.shield(spawning)
                    break
                except asyncio.CancelledError:
                    # An observer may be cancelled again while ownership is
                    # still being acquired. Never cancel the creation task or
                    # release admission before its exact handle is available.
                    cancelled = True
                    if spawning.cancelled():
                        # An independently cancelled creator has unresolved
                        # custody. It cannot become an ordinary failed query.
                        self._unresolved_spawn = True
                        raise ProbeUnavailable("Owned CLI spawn custody could not be resolved.") from None
            if cancelled:
                raise asyncio.CancelledError
            async def bounded(stream, limit, retain):
                content = bytearray()
                size = 0
                while chunk := await stream.read(4096):
                    size += len(chunk)
                    if size > limit:
                        raise ProbeUnavailable("The cloud query exceeded its output bound.")
                    if retain:
                        content.extend(chunk)
                return bytes(content)
            readers = [asyncio.create_task(bounded(process.stdout, MAX_TOKEN if secret else self.output_limit, True)),
                       asyncio.create_task(bounded(process.stderr, MAX_ERROR, False))]
            async def finish():
                output, _ = await asyncio.gather(*readers)
                await process.wait()
                return CommandResult(process.returncode, output.decode("utf-8", errors="strict"))
            return await asyncio.wait_for(finish(), self.timeout)
        except (TimeoutError, UnicodeError, OSError) as exc:
            raise ProbeUnavailable("The cloud query could not be observed within its bounds.") from None
        finally:
            async def cleanup():
                if process is not None:
                    # Parent exit does not prove the pipe-owning descendants
                    # stopped. Retain both group and parent observations.
                    self._pending_groups.add(process.pid)
                    try:
                        os.killpg(process.pid, signal.SIGKILL)
                    except (ProcessLookupError, PermissionError):
                        pass
                for reader in readers:
                    if not reader.done():
                        reader.cancel()
                if readers:
                    await asyncio.gather(*readers, return_exceptions=True)
                if process is not None:
                    # Overflow readers may leave asyncio's pipe buffer paused.
                    # Drain without retaining private bytes so a killed child's
                    # EOF can close its transport and complete the parent wait.
                    async def drain(stream):
                        while await stream.read(4096):
                            pass
                    drains = [asyncio.create_task(drain(stream)) for stream in (process.stdout, process.stderr) if stream]
                    waiter = asyncio.create_task(process.wait())
                    self._pending_processes[process.pid] = (process, waiter)
                    try:
                        await asyncio.wait_for(asyncio.shield(asyncio.gather(waiter, *drains)), 2)
                    except TimeoutError:
                        pass
                    finally:
                        for drain_task in drains:
                            if not drain_task.done():
                                drain_task.cancel()
                        await asyncio.gather(*drains, return_exceptions=True)
                until = time.monotonic() + 2
                while self.cleanup_pending and time.monotonic() < until:
                    await asyncio.sleep(0.02)
                if self.cleanup_pending:
                    raise ProbeUnavailable("An owned CLI group has not positively stopped; no new cloud probe can be admitted.")
            owned_cleanup = asyncio.create_task(cleanup())
            try:
                await asyncio.shield(owned_cleanup)
            except asyncio.CancelledError:
                # Keep custody through a second observer cancellation too.
                while not owned_cleanup.done():
                    try:
                        await asyncio.shield(owned_cleanup)
                    except asyncio.CancelledError:
                        continue
                await owned_cleanup
                raise


class GoogleReadOnlyRest:
    """Fixed HTTPS reads with a total timeout and no redirect credential forwarding."""
    def __init__(self, *, transport=None, timeout=COMMAND_TIMEOUT):
        self.transport, self.timeout = transport, timeout

    async def request(self, method, url, body, token):
        parsed = parse.urlsplit(url)
        if (parsed.scheme != "https" or parsed.hostname not in {
                "cloudresourcemanager.googleapis.com", "serviceusage.googleapis.com",
                "iap.googleapis.com", "compute.googleapis.com", "orgpolicy.googleapis.com",
                "openidconnect.googleapis.com", "cloudbilling.googleapis.com"}
                or parsed.username or parsed.password or parsed.port or parsed.fragment
                or method not in {"GET", "POST"}):
            raise ProbeUnavailable("The cloud observation endpoint is unsupported.")
        if method == "POST" and not parsed.path.endswith(":testIamPermissions"):
            raise ProbeUnavailable("Only permission observations can use POST.")
        encoded = json.dumps(body, separators=(",", ":")).encode() if body is not None else None
        if encoded is not None and len(encoded) > 16 * 1024:
            raise ProbeUnavailable("The permission observation exceeded its request bound.")
        try:
            async with asyncio.timeout(self.timeout):
                # trust_env=False excludes proxy and credential environment
                # overrides; an HTTP redirect is returned as unknown, never
                # followed with the selected account's bearer credential.
                async with httpx.AsyncClient(transport=self.transport, timeout=self.timeout,
                        follow_redirects=False, trust_env=False) as client:
                    async with client.stream(method, url, content=encoded,
                            headers={"Authorization": "Bearer " + token,
                                     "Content-Type": "application/json"}) as response:
                        if response.status_code != 200:
                            return RestResult(response.status_code, None)
                        data = bytearray()
                        async for chunk in response.aiter_bytes():
                            if len(data) + len(chunk) > MAX_OUTPUT:
                                raise ProbeUnavailable("The cloud observation exceeded its output bound.")
                            data.extend(chunk)
                        return RestResult(response.status_code, json.loads(data))
        except asyncio.CancelledError:
            raise
        except (httpx.HTTPError, ValueError, OSError, TimeoutError):
            raise ProbeUnavailable("The cloud observation could not be verified within its bounds.") from None


def _check(identity, label, status, detail, *, action=None, required=True):
    result = {"id": identity, "label": label, "status": status, "detail": detail, "required": required}
    if action:
        result["action"] = action
    return result


def _json(value):
    if not isinstance(value, str) or len(value.encode()) > MAX_OUTPUT:
        raise ProbeUnavailable("The cloud query returned invalid bounded JSON.")
    try:
        return json.loads(value)
    except (ValueError, RecursionError):
        raise ProbeUnavailable("The cloud query returned invalid bounded JSON.") from None


def _account(value):
    if not isinstance(value, str) or not ACCOUNT.fullmatch(value):
        raise ValueError("Choose an exact signed-in Google Cloud account email.")
    return value


def _project(value):
    if not isinstance(value, str) or not PROJECT.fullmatch(value):
        raise ValueError("Choose the exact Google Cloud project ID.")
    return value


def _resource_matches(value, suffix):
    return isinstance(value, str) and (value == suffix.lstrip("/") or value.endswith(suffix))


class GcpPreflightService:
    def __init__(self, local_deployment, *, command=None, rest=None):
        self.local = local_deployment
        self.command = command if command is not None else GcloudReadOnly()
        self.rest = rest if rest is not None else GoogleReadOnlyRest()
        self._context = dict(getattr(self.command, "environment", _environment()))
        self._active = None
        self._epoch = 0
        self._closed = False
        self._operation_finished = False

    async def _snapshot(self, guide_id, build_id, version=None):
        status = await self.local.implementation.status(guide_id)
        require_accepted(status)
        if (status.get("build_id") != build_id or status.get("guide_id") != guide_id
                or status.get("current") is not True or status.get("ready_to_package") is not True
                or not isinstance(status.get("source_digest"), str) or not SHA.fullmatch(status["source_digest"])):
            raise DraftConflict("The selected application source is no longer accepted and current. Rebuild before cloud preparation.")
        state = await self.local._guide(guide_id)
        if (state.get("id") != guide_id or type(state.get("version")) is not int
                or (version is not None and (type(version) is not int or state["version"] != version))):
            raise DraftConflict("The setup changed. Reload cloud preparation before checking a destination.")
        proposal = Blueprint.model_validate(state.get("proposal") or state.get("blueprint"))
        if proposal.target != "gcp":
            raise ValueError("Choose Google Cloud with the setup guide and accept its application package first.")
        if proposal.gcp.provisioning == "existing_host":
            raise ValueError("This proposal uses an existing host. New-VM cloud preparation is unavailable; use the existing destination's installation controls or package handoff.")
        payload, slug = await self.local.implementation.package(guide_id, build_id)
        # Unpacking and hashing the whole package must not hold the event loop.
        files = await await_validation(package_files, payload)
        try:
            definition = json.loads(files["deployment.json"][0])
            report = json.loads(files["implementation-report.json"][0])
            if not isinstance(definition, dict) or not isinstance(report, dict):
                raise ValueError("Deployment evidence must be JSON objects.")
            verification = report.get("verification", {})
            if not isinstance(verification, dict):
                raise ValueError("Verification evidence must be a JSON object.")
            knowledge = definition.get("knowledge")
            if knowledge is not None and not isinstance(knowledge, dict):
                raise ValueError("Knowledge evidence must be a JSON object.")
        except (KeyError, ValueError):
            raise DraftConflict("The accepted cloud package has invalid deployment evidence.") from None
        if (report.get("build_id") != build_id or not package_matches(report, state)
                or verification.get("digest") != status["source_digest"]
                or definition.get("target") != "gcp" or definition.get("slug") != proposal.project.slug
                or slug != proposal.project.slug or definition.get("port") != proposal.advanced.port):
            raise DraftConflict("The accepted package differs from the current proposal or checked source. Rebuild before cloud preparation.")
        destination = {"project": proposal.gcp.project, "zone": proposal.gcp.zone,
            "machine_type": proposal.gcp.machine_type, "instance": proposal.project.slug,
            "console_port": proposal.advanced.port, "disk_gb": proposal.gcp.disk_gb}
        # A loose agent-authored application may customize ordinary code, but an
        # arbitrary accepted deploy script cannot inherit this fixed cloud plan.
        from .generator import TEMPLATES
        from .compiler import TEMPLATES_V2
        from .deployment_deadlines import render_destination_shutdown_budget
        deploy = (TEMPLATES / "deploy-gcp.sh").read_text().replace("30 GB disk", f"{proposal.gcp.disk_gb} GB disk").replace("--boot-disk-size 30GB", f"--boot-disk-size {proposal.gcp.disk_gb}GB")
        for key, value in {"__PROJECT__": shlex.quote(proposal.gcp.project), "__ZONE__": shlex.quote(proposal.gcp.zone),
                "__INSTANCE__": shlex.quote(proposal.project.slug), "__MACHINE__": shlex.quote(proposal.gcp.machine_type),
                "__PORT__": str(proposal.advanced.port)}.items():
            deploy = deploy.replace(key, value)
        tunnel = f"#!/usr/bin/env bash\nset -euo pipefail\nexec gcloud compute ssh {shlex.quote(proposal.project.slug)} --project {shlex.quote(proposal.gcp.project)} --zone {shlex.quote(proposal.gcp.zone)} --tunnel-through-iap -- -N -L {proposal.advanced.port}:127.0.0.1:{proposal.advanced.port}\n"
        from .gcp_bootstrap import render_manual_prerequisites
        # Retained accepted packages may use the earlier fixed export. Neither
        # form grants arbitrary agent-authored scripts this adapter's authority.
        deploy_versions = {deploy.encode(), render_manual_prerequisites(deploy, definition).encode()}
        # A package Forge 0.9.4 accepted keeps that release's exact helper.
        destination_helper = files.get("remote_operations.py", (None,))[0]
        helper_supported = destination_helper is not None and (
            destination_helper == render_destination_shutdown_budget((TEMPLATES_V2 / "remote_operations.py").read_text()).encode()
            or hashlib.sha256(destination_helper).hexdigest() in RETAINED_DESTINATION_HELPER_SHA256)
        supported = bool(files.get("deploy/deploy-gcp.sh", (None,))[0] in deploy_versions
            and files.get("deploy/tunnel-gcp.sh", (None,))[0] == tunnel.encode()
            and files.get("deploy/install-service.sh", (None,))[0] == (TEMPLATES / "install-service.sh").read_bytes()
            and helper_supported)
        digest = await await_validation(lambda: hashlib.sha256(payload).hexdigest())
        binding = {"guide_id": guide_id, "guide_version": state["version"], "build_id": build_id,
            "blueprint_hash": proposal_hash(state.get("proposal") or state.get("blueprint")),
            "source_digest": status["source_digest"], "package_sha256": digest}
        return binding, destination, supported

    def _admit(self):
        if self._closed:
            raise DraftConflict("Cloud preparation is shutting down.")
        # Every requested observation invalidates an earlier presentation, even
        # account A -> B -> A. The old native probe still owns its slot.
        self._epoch += 1
        if self._active is not None and self._operation_finished and not getattr(self.command, "cleanup_pending", False):
            self._active = None
        if self._active is not None or getattr(self.command, "cleanup_pending", False):
            raise DraftConflict("A cloud check is still finishing. Wait for its owned probe to stop, then check again.")
        self._active = asyncio.current_task()
        self._operation_finished = False
        return self._epoch

    async def _run(self, guide_id, build_id, version, operation):
        epoch = self._admit()
        try:
            async def observe():
                initial = await self._snapshot(guide_id, build_id, version)
                result = await operation(initial)
                latest = await self._snapshot(guide_id, build_id, initial[0]["guide_version"])
                if epoch != self._epoch or latest != initial:
                    raise DraftConflict("The setup, selected account or project changed while this cloud check was running. Check the current selection again.")
                return {"protocol": 1, "binding": initial[0], "destination": initial[1],
                        "checked_at": datetime.now(timezone.utc).isoformat(), **result}
            return await asyncio.wait_for(observe(), PROBE_TIMEOUT)
        except TimeoutError:
            raise DraftConflict("The bounded cloud check timed out. No cloud resources were changed.") from None
        finally:
            self._operation_finished = True
            if self._active is asyncio.current_task() and not getattr(self.command, "cleanup_pending", False):
                self._active = None

    async def shutdown(self):
        self._closed = True
        self._epoch += 1
        active = self._active
        if active is not None and active is not asyncio.current_task():
            active.cancel()
            await asyncio.gather(active, return_exceptions=True)
        if getattr(self.command, "cleanup_pending", False):
            raise DraftConflict("An owned cloud CLI group has not positively stopped. Its admission remains reserved.")

    async def _cli(self, parts, account=None, project=None, *, secret=False):
        args = [*parts, "--quiet", "--verbosity=error", "--no-log-http"]
        if account:
            args.append("--account=" + account)
        if project:
            args.append("--project=" + project)
        try:
            result = await self.command.command(args, secret=secret)
            if not isinstance(result, CommandResult):
                result = CommandResult(*result)
            if result.code != 0:
                raise ProbeUnavailable("The selected account's cloud query could not be verified.")
            return result.output if secret else _json(result.output)
        except asyncio.CancelledError:
            raise
        except (ProbeUnavailable, TimeoutError, ValueError, TypeError, OSError):
            raise ProbeUnavailable("The selected account's cloud query could not be verified.") from None

    async def _accounts(self, selected=None):
        if not self.command.available:
            return [], [_check("cli", "Google Cloud CLI", "blocked", "Google Cloud CLI was not detected when Forge started.", action="Install Google Cloud CLI on this computer and sign in, then restart Forge and reopen this saved setup before checking cloud access again.")], False
        try:
            # A projection of unset override fields serializes as null even
            # for an ordinary configured CLI. Read the complete section object.
            profile = await self._cli(["config", "list", "--format=json"], selected)
            if not isinstance(profile, dict):
                raise ProbeUnavailable()
            auth = profile.get("auth", {})
            if not isinstance(auth, dict):
                raise ProbeUnavailable()
            core = profile.get("core", {})
            if not isinstance(core, dict):
                raise ProbeUnavailable()
            endpoints = profile.get("api_endpoint_overrides", {})
            if not isinstance(endpoints, dict):
                raise ProbeUnavailable()
            overrides = []
            for key in ("impersonate_service_account", "credential_file_override", "access_token_file", "access_token"):
                if key not in auth:
                    continue
                value = auth[key]
                if not isinstance(value, str):
                    raise ProbeUnavailable()
                if value:
                    overrides.append(key)
            for section, key in ((auth, "disable_credentials"), (auth, "disable_ssl_validation"), (core, "disable_ssl_validation")):
                if key not in section:
                    continue
                value = section[key]
                if type(value) is bool:
                    enabled = value
                elif isinstance(value, str) and value in ("false", "False", "true", "True"):
                    enabled = value in ("true", "True")
                else:
                    raise ProbeUnavailable()
                if enabled:
                    overrides.append(key)
            if overrides or endpoints:
                return [], [_check("account-binding", "Selected account identity", "blocked", "This CLI configuration uses credential, impersonation, endpoint or TLS overrides. It cannot prove that the selected account is the effective cloud principal.", action="Use an ordinary signed-in CLI configuration without these overrides, then check again.")], False
            rows = await self._cli(["auth", "list", "--limit=101", "--format=json(account,status)"], selected)
            if not isinstance(rows, list) or len(rows) > MAX_ACCOUNTS + 1:
                raise ProbeUnavailable()
            accounts = []
            seen = set()
            for row in rows:
                if not isinstance(row, dict) or not ACCOUNT.fullmatch(str(row.get("account", ""))) or row.get("status") not in {"ACTIVE", ""}:
                    raise ProbeUnavailable()
                identity = row["account"]
                if identity in seen:
                    raise ProbeUnavailable()
                seen.add(identity)
                accounts.append({"id": identity, "label": identity, "active": row["status"] == "ACTIVE"})
            truncated = len(accounts) > MAX_ACCOUNTS
            if selected and selected not in seen:
                return accounts[:MAX_ACCOUNTS], [_check("account", "Signed-in account", "blocked", "The selected account is not present in this CLI's bounded account inventory.", action="Sign in to that account separately with Google Cloud CLI, then reload accounts.")], truncated
            return accounts[:MAX_ACCOUNTS], [_check("cli", "Google Cloud CLI", "checked", "The installed CLI returned a valid bounded account inventory."),
                _check("account-binding", "Selected account identity", "checked", "CLI credential and endpoint overrides are absent; subsequent cloud queries explicitly select the account.")], truncated
        except ProbeUnavailable:
            return [], [_check("cli", "Google Cloud CLI", "unknown", "The CLI or account configuration could not be read within its bounds. No private error output is shown.", action="Check Google Cloud CLI locally and retry.")], False

    @staticmethod
    def _package_check(supported):
        return _check("package-deployer", "Accepted deployment scripts", "checked" if supported else "blocked",
            "Deployment and tunnel scripts exactly match the supported package-bound cloud adapter." if supported else "This accepted package customizes or lacks the supported deployment scripts. A fixed cloud plan cannot be inferred from that code.",
            action=None if supported else "Ask the implementation agent to restore or explicitly support the customized deployer, then accept the new package.")

    async def options(self, guide_id, build_id):
        async def operation(snapshot):
            _, _, supported = snapshot
            accounts, checks, truncated = await self._accounts()
            cli_status = "blocked" if not self.command.available else "checked" if accounts or all(row["status"] == "checked" for row in checks) else checks[0]["status"]
            if not accounts and all(row["status"] == "checked" for row in checks):
                checks.append(_check("account", "Signed-in account", "blocked", "No signed-in Google Cloud account is available.", action="Sign in separately with Google Cloud CLI, then reload accounts."))
            return {"cli": {"available": bool(self.command.available), "status": cli_status,
                    "detail": "Google Cloud CLI is installed." if self.command.available else "Google Cloud CLI was not detected when Forge started."},
                "accounts": accounts, "truncated": truncated, "checks": [self._package_check(supported), *checks]}
        return await self._run(guide_id, build_id, None, operation)

    async def _token(self, account, project, checks):
        try:
            token = (await self._cli(["auth", "print-access-token"], account, project, secret=True)).strip()
            if not token or len(token) > MAX_TOKEN or any(ord(c) <= 32 or ord(c) >= 127 for c in token):
                raise ProbeUnavailable()
            principal = await self._rest("GET", "https://openidconnect.googleapis.com/v1/userinfo", None, token)
            if principal.get("email_verified") is not True or not isinstance(principal.get("email"), str):
                raise ProbeUnavailable()
            if principal["email"] != account:
                checks.append(_check("effective-account", "Effective cloud account", "blocked",
                    "The transient cloud credential identifies a different principal from the selected account. No account substitution is accepted.",
                    action="Use an ordinary signed-in account without credential overrides, then check again."))
                return None
            # Cloud checks below all use this exact immutable verified token.
            # A subsequent CLI configuration change cannot switch their caller.
            checks.append(_check("effective-account", "Effective cloud account", "checked",
                "Google's authenticated identity endpoint verified the selected account for the exact transient token used by the cloud observations. The token and other identity fields are not exposed."))
            return token
        except ProbeUnavailable:
            checks.append(_check("effective-account", "Effective cloud account", "unknown",
                "The transient cloud credential's selected-account identity could not be verified. This account type or its scopes may require another supported authentication adapter.",
                action="Check the signed-in account and its identity scopes, then retry."))
            return None

    async def _final_account_check(self, account, checks):
        _, current, _ = await self._accounts(account)
        status = "blocked" if any(row["status"] == "blocked" for row in current) else "unknown" if any(row["status"] == "unknown" for row in current) else "checked"
        checks.append(_check("account-current", "Current CLI account configuration", status,
            "The selected account is still signed in and the ordinary CLI configuration remains supported after cloud observations." if status == "checked" else
            "The selected account or CLI credential configuration is no longer supported or could not be rechecked. Its earlier cloud observations cannot authorize a manual deployment plan.",
            action=None if status == "checked" else "Restore the selected ordinary account configuration and check again."))

    async def projects(self, guide_id, version, build_id, account):
        account = _account(account)
        async def operation(snapshot):
            _, destination, supported = snapshot
            _, checks, _ = await self._accounts(account)
            rows, truncated = [], False
            if all(check["status"] == "checked" for check in checks):
                token = await self._token(account, destination["project"], checks)
                if token:
                    try:
                        inventory = await self._rest("GET", "https://cloudresourcemanager.googleapis.com/v1/projects?" + parse.urlencode({"pageSize": MAX_PROJECTS, "filter": "lifecycleState:ACTIVE"}), None, token)
                        projects = inventory.get("projects", [])
                        if not isinstance(projects, list) or len(projects) > MAX_PROJECTS or not isinstance(inventory.get("nextPageToken", ""), str):
                            raise ProbeUnavailable()
                        seen = set()
                        for row in projects:
                            if not isinstance(row, dict) or not PROJECT.fullmatch(str(row.get("projectId", ""))) or row.get("lifecycleState") != "ACTIVE":
                                raise ProbeUnavailable()
                            identity = row["projectId"]
                            if identity in seen:
                                raise ProbeUnavailable()
                            seen.add(identity)
                            label = row.get("name", identity)
                            if not isinstance(label, str) or len(label) > 200 or any(ord(c) < 32 for c in label):
                                raise ProbeUnavailable()
                            rows.append({"id": identity, "label": label})
                        truncated = bool(inventory.get("nextPageToken"))
                        checks.append(_check("project-discovery", "Available projects", "checked", "Returned one bounded page of project choices. Enter an exact project ID if it is omitted; the next check independently describes that project.", required=False))
                    except ProbeUnavailable:
                        rows = []
                        checks.append(_check("project-discovery", "Available projects", "unknown", "Projects could not be listed. This does not prove the account has no projects; an exact project ID can still be checked.", required=False))
                    finally:
                        token = None
                    await self._final_account_check(account, checks)
            return {"account": account, "projects": rows, "truncated": truncated,
                    "checks": [self._package_check(supported), *checks]}
        return await self._run(guide_id, build_id, version, operation)

    async def _rest(self, method, url, body, token):
        try:
            result = await self.rest.request(method, url, body, token)
            if not isinstance(result, RestResult):
                result = RestResult(*result)
            if result.status != 200 or not isinstance(result.value, dict):
                raise ProbeUnavailable()
            if len(json.dumps(result.value).encode()) > MAX_OUTPUT:
                raise ProbeUnavailable()
            return result.value
        except asyncio.CancelledError:
            raise
        except (ProbeUnavailable, OSError, TimeoutError, ValueError, TypeError, RecursionError):
            raise ProbeUnavailable("The cloud observation could not be verified.") from None

    async def check(self, guide_id, version, build_id, account, project, *, _guided=False):
        account, project = _account(account), _project(project)
        local_knowledge = False
        async def operation(snapshot):
            nonlocal local_knowledge
            binding, destination, supported = snapshot
            checks = [self._package_check(supported)]
            matching = project == destination["project"]
            checks.append(_check("project-match", "Accepted project", "checked" if matching else "blocked",
                "The selected project matches the accepted package." if matching else "The selected project differs from the immutable accepted package. Changing the picker cannot redirect its scripts.",
                action=None if matching else "Send this project choice to the setup guide, rebuild the application and accept its new package."))
            if not matching or not supported:
                return self._result(account, project, destination, checks)
            if _guided:
                payload, _ = await self.local.implementation.package(guide_id, build_id)
                if await await_validation(lambda: hashlib.sha256(payload).hexdigest()) != binding["package_sha256"]:
                    raise DraftConflict("The accepted package changed before guest preparation. No cloud resources were requested.")
                definition = json.loads((await await_validation(package_files, payload))["deployment.json"][0])
                local_knowledge = (definition.get("knowledge") or {}).get("mode", "none") == "local"
            _, account_checks, _ = await self._accounts(account)
            checks.extend(account_checks)
            if any(row["status"] != "checked" for row in account_checks):
                return self._result(account, project, destination, checks)
            token = await self._token(account, project, checks)
            if not token:
                return self._result(account, project, destination, checks)
            try:
                described = await self._rest("GET", "https://cloudresourcemanager.googleapis.com/v1/projects/" + project, None, token)
                number = described.get("projectNumber")
                if (described.get("projectId") != project or described.get("lifecycleState") != "ACTIVE"
                        or not isinstance(number, str) or not re.fullmatch(r"[0-9]{1,24}", number)):
                    raise ProbeUnavailable()
                checks.append(_check("project", "Project access", "checked", "The verified selected-account token independently described this exact active project."))
            except ProbeUnavailable:
                token = None
                checks.append(_check("project", "Project access", "unknown", "The exact active project could not be verified. A denied or malformed response is not evidence of absence.", action="Check the selected account's project access, then retry."))
                return self._result(account, project, destination, checks)
            try:
                billing = await self._rest("GET", "https://cloudbilling.googleapis.com/v1/projects/" + project + "/billingInfo", None, token)
                if billing.get("projectId") != project or type(billing.get("billingEnabled")) is not bool:
                    raise ProbeUnavailable()
                enabled = billing["billingEnabled"]
                checks.append(_check("billing", "Project billing", "checked" if enabled else "blocked",
                    "Billing is enabled on this project." if enabled else "Billing is not enabled on this project.",
                    action=None if enabled else "Enable billing in Google Cloud separately, then check again."))
            except ProbeUnavailable:
                checks.append(_check("billing", "Project billing", "unknown", "Billing could not be verified; an inaccessible billing record is not a disabled billing account.", action="Ask a billing administrator to check this project, then retry."))
            try:
                await self._cloud_checks(destination, number, token, checks)
                if _guided:
                    await self._guided_access_checks(project, number, token, checks)
                    await self._guided_zonal_ssh_check(project, destination["zone"], token, checks)
            finally:
                token = None
            await self._final_account_check(account, checks)
            return self._result(account, project, destination, checks)
        result = await self._run(guide_id, build_id, version, operation)
        if _guided:
            eligible = {"package-deployer", "ssh-mode", "policy-requireOsLogin", "guest-key-permission",
                "policy-disableGuestAttributesAccess", "policy-managed.disableGuestAttributesAccess",
                "policy-managed.requireOsLogin", "zonal-ssh-mode"}
            by_id = {row["id"]: row for row in result["checks"]}
            # Existing owned resource collisions do not change transport eligibility.
            # The coordinator must separately prove their exact generation before
            # disregarding any collision in a continuation preflight.
            result["guided_supported"] = all(by_id.get(key, {}).get("status") == "checked" for key in eligible)
            result["guided_access"] = {"mode": "metadata-ssh-guest-host-key", "user": "mobkit",
                "host_key_source": "authenticated-compute-guest-attributes",
                "guest_attributes_enabled_before_boot": True,
                "private_settings_on_host_form": True}
            if result.get("plan") is not None:
                result["plan"].update(title="Guided cloud computer preparation", commands=[],
                    access=["Verify the new VM's published SSH host key through authenticated Google Cloud access.",
                        "Reach the VM and application Console through an owned local IAP SSH tunnel.",
                        "Enter private application credentials in Forge's private setup form after the computer is verified.",
                        "Existing network/firewall rules still apply and can admit broader SSH access."],
                    mutations=["Create the listed billed VM, boot disk and scoped IAP SSH firewall rule in the selected project.",
                        "Enable guest host-key publication at creation and prepare Python, virtual environments and user-service support.",
                        "Create a private SSH identity on this computer and publish only its public key in VM metadata.",
                        "Application source transfer, private configuration and service startup require the separate installation action.",
                        "Stopping the application does not delete cloud resources or stop VM and disk charges."],
                    limitations=["Cloud authorizes every actual operation. Permissions and quotas are observations; no capacity is reserved.",
                        "The fixed guest bootstrap prepares Python and user services. A local knowledge service and additional connector executables may require further host preparation; application installation is blocked until its prerequisite checks pass.",
                        "A verified VM is not an installed or healthy application. Native startup and a useful application result are checked later.",
                        "Retain the original setup request for ownership-checked recovery after an interrupted creation."])
                if local_knowledge:
                    result["plan"]["mutations"].append(
                        "For a selected local knowledge service, install Git, Docker Engine and the Compose/build plugins from Docker's official Debian repository. Grant the mobkit installation user Docker-group access, which grants root-equivalent control of this VM. Apply this access to that user's service manager, starting it if absent or restarting it only when its observed groups lack Docker access.")
                    result["plan"]["limitations"][1] = (
                        "The guest bootstrap prepares the selected local knowledge tools. Docker access, Compose wait support and Git are checked before installation. Repository access, credentials, the source build and actual knowledge health remain installation checks; no knowledge service is started by VM preparation.")
        return result

    async def check_guided(self, guide_id, version, build_id, account, project):
        """Observe the explicit fresh-VM host-key extension without provisioning."""
        return await self.check(guide_id, version, build_id, account, project, _guided=True)

    async def _guided_zonal_ssh_check(self, project, zone, token, checks):
        try:
            value = await self._rest("GET", "https://compute.googleapis.com/compute/v1/projects/" + project +
                "/zones/" + zone + "/instanceSettings", None, token)
            metadata = value.get("metadata", {}) if isinstance(value, dict) else None
            items = metadata.get("items", {}) if isinstance(metadata, dict) else None
            if (not isinstance(value, dict) or value.get("kind") not in ("compute#instance_settings", "compute#instanceSettings")
                    or not _resource_matches(value.get("zone"), "/projects/" + project + "/zones/" + zone)
                    or not isinstance(value.get("fingerprint"), str) or not 1 <= len(value["fingerprint"]) <= 512
                    or not isinstance(items, dict) or len(items) > 1024
                    or any(not isinstance(key, str) or not 1 <= len(key) <= 128 or not isinstance(item, str) for key, item in items.items())):
                raise ProbeUnavailable()
            login = items.get("enable-oslogin", "false").strip().lower()
            if login not in {"false", "true"}:
                raise ProbeUnavailable()
            denied = login == "true"
            checks.append(_check("zonal-ssh-mode", "Zonal SSH sign-in settings", "blocked" if denied else "checked",
                "OS Login is enabled at this zone. The supported metadata-SSH installer cannot connect without changing that setting." if denied else
                "The exact zonal metadata permits metadata SSH; the checked project and effective policies still apply.",
                action="Use an administrator-approved deployment path; Forge will not disable OS Login." if denied else None))
        except ProbeUnavailable:
            checks.append(_check("zonal-ssh-mode", "Zonal SSH sign-in settings", "unknown",
                "The selected zone's SSH metadata could not be verified. No override was assumed absent.",
                action="Ask the project administrator to check compute.instanceSettings.get access, then retry."))

    async def _guided_access_checks(self, project, number, token, checks):
        permission = "compute.instances.getGuestAttributes"
        try:
            value = await self._rest("POST", "https://cloudresourcemanager.googleapis.com/v1/projects/" + project + ":testIamPermissions",
                {"permissions": [permission]}, token)
            rows = value.get("permissions", []) if isinstance(value, dict) else None
            if not isinstance(rows, list) or rows not in ([], [permission]):
                raise ProbeUnavailable()
            checks.append(_check("guest-key-permission", "Verify the new computer's SSH identity", "checked" if rows else "blocked",
                "Guest host-key read permission was observed at project scope; actual VM access still needs verification." if rows else
                "The selected account cannot yet read the new computer's published SSH host key.",
                action=None if rows else "Ask the project administrator to grant compute.instances.getGuestAttributes, then check again."))
        except ProbeUnavailable:
            checks.append(_check("guest-key-permission", "Verify the new computer's SSH identity", "unknown",
                "Guest host-key read permission could not be verified. Private settings will not be sent without a verified SSH identity."))
        for name, label in (("disableGuestAttributesAccess", "Guest host-key publication policy"),
                ("managed.disableGuestAttributesAccess", "Managed guest host-key publication policy"),
                ("managed.requireOsLogin", "Managed SSH sign-in policy")):
            expected = "projects/" + number + "/policies/compute." + name
            try:
                value = await self._rest("GET", "https://orgpolicy.googleapis.com/v2/" + expected + ":getEffectivePolicy", None, token)
                spec = value.get("spec") if isinstance(value, dict) else None
                rules = spec.get("rules") if isinstance(spec, dict) else None
                if (not isinstance(value, dict) or value.get("name") != expected or not isinstance(rules, list) or len(rules) != 1
                        or not isinstance(rules[0], dict) or "condition" in rules[0]
                        or type(rules[0].get("enforce")) is not bool):
                    raise ProbeUnavailable()
                denied = rules[0]["enforce"]
                checks.append(_check("policy-" + name, label, "blocked" if denied else "checked",
                    "This effective policy prevents the supported metadata-SSH host-key setup." if denied else
                    "The effective policy permits this supported metadata-SSH host-key setup.",
                    action="Use an administrator-approved supported deployment path; this installer will not disable the policy." if denied else None))
            except ProbeUnavailable:
                checks.append(_check("policy-" + name, label, "unknown",
                    "This effective policy is unavailable, conditional or incomplete. The supported host-key setup cannot be proven permitted."))

    async def _cloud_checks(self, d, number, token, checks):
        project, zone, machine = d["project"], d["zone"], d["machine_type"]
        region = zone.rsplit("-", 1)[0]
        base = "https://compute.googleapis.com/compute/v1/projects/" + project
        async def inspect(identity, label, url, validate, *, method="GET", body=None, required=True):
            try:
                value = await self._rest(method, url, body, token)
                status, detail, action = validate(value)
                if status not in {"checked", "blocked", "unknown"}:
                    raise ProbeUnavailable()
                checks.append(_check(identity, label, status, detail, action=action, required=required))
                return value if status == "checked" else None
            except (ProbeUnavailable, KeyError, TypeError, ValueError, AttributeError, OverflowError):
                checks.append(_check(identity, label, "unknown", "This bounded cloud observation could not be verified. Denied, incomplete and malformed responses remain unknown.", required=required,
                    action="Check access to this cloud information and retry."))
                return None
        def apis(value):
            rows = value.get("services")
            if not isinstance(rows, list) or len(rows) != len(SERVICES):
                raise ProbeUnavailable()
            names = {"projects/" + number + "/services/" + service for service in SERVICES}
            if {row.get("name") for row in rows if isinstance(row, dict)} != names or any(row.get("state") not in {"ENABLED", "DISABLED"} for row in rows):
                raise ProbeUnavailable()
            disabled = [row["name"].rsplit("/", 1)[1] for row in rows if row["state"] != "ENABLED"]
            return ("blocked", "Required APIs are disabled: " + ", ".join(disabled) + ".", "Enable the selected APIs separately, then check again.") if disabled else ("checked", "Compute Engine and IAP APIs are both enabled.", None)
        query = parse.urlencode([("names", "projects/" + number + "/services/" + service) for service in SERVICES]
            + [("fields", "services(name,state)")])
        await inspect("apis", "Compute Engine and IAP APIs", "https://serviceusage.googleapis.com/v1/projects/" + number + "/services:batchGet?" + query, apis)
        def permissions(value):
            rows = value.get("permissions", []) if isinstance(value, dict) else None
            if not isinstance(rows, list) or any(not isinstance(v, str) or v not in PERMISSIONS for v in rows) or len(set(rows)) != len(rows):
                raise ProbeUnavailable()
            missing = [v for v in PERMISSIONS if v not in rows]
            return ("blocked", "Project-level permission observation did not return: " + ", ".join(missing) + ".", "Ask the project administrator to review deployment and metadata SSH permissions. Conditional resource grants may require a customized adapter.") if missing else ("checked", "The selected principal returned the fixed deployment and metadata SSH permissions at project scope. This is advisory, not authorization of future mutations.", None)
        await inspect("permissions", "Deployment permissions", "https://cloudresourcemanager.googleapis.com/v1/projects/" + project + ":testIamPermissions", permissions, method="POST", body={"permissions": list(PERMISSIONS)})
        def iap(value):
            rows = value.get("permissions", []) if isinstance(value, dict) else None
            if not isinstance(rows, list) or any(v != IAP_PERMISSION for v in rows) or len(rows) > 1:
                raise ProbeUnavailable()
            return ("checked", "IAP tunnel access was observed at project-parent scope. A future VM, port 22 and conditional policy still require actual authorization.", None) if rows else ("unknown", "No IAP tunnel permission was observed at project-parent scope. This cannot prove access or denial for the future VM and its conditional policies.", "Ask the project administrator to verify IAP SSH access for the proposed VM.")
        await inspect("iap-permission", "IAP SSH permission", "https://iap.googleapis.com/v1/projects/" + number + "/iap_tunnel:testIamPermissions", iap, method="POST", body={"permissions": [IAP_PERMISSION]})
        def zone_info(value):
            if value.get("name") != zone or not _resource_matches(value.get("region"), "/projects/" + project + "/regions/" + region) or value.get("status") not in {"UP", "DOWN"}:
                raise ProbeUnavailable()
            return ("checked", "The accepted zone is available for planning; live instance capacity is not reserved.", None) if value["status"] == "UP" else ("blocked", "The accepted zone reports DOWN.", "Choose another zone with the setup guide and rebuild.")
        await inspect("zone", "Accepted zone", base + "/zones/" + zone, zone_info)
        def machine_info(value):
            if (value.get("name") != machine or value.get("zone") != zone
                    or type(value.get("guestCpus")) is not int or not 1 <= value["guestCpus"] <= 1024
                    or type(value.get("memoryMb")) is not int or value["memoryMb"] <= 0):
                raise ProbeUnavailable()
            return "checked", f"The accepted machine type has {value['guestCpus']} guest CPUs and {value['memoryMb']} MB of memory. This does not prove application sizing or live capacity.", None
        shape = await inspect("machine", "Accepted machine type", base + "/zones/" + zone + "/machineTypes/" + machine, machine_info)
        def network_info(value):
            if value.get("name") != "default" or not _resource_matches(value.get("selfLink"), "/projects/" + project + "/global/networks/default"):
                raise ProbeUnavailable()
            if value.get("autoCreateSubnetworks") is not True:
                return "unknown", "The default VPC is not an automatic subnet network. The fixed script does not choose a custom subnet explicitly.", "Ask the guide to design and support the project's actual network before rebuilding."
            expected = "/projects/" + project + "/regions/" + region + "/subnetworks/default"
            subnets = value.get("subnetworks")
            if not isinstance(subnets, list) or not any(_resource_matches(v, expected) for v in subnets):
                raise ProbeUnavailable()
            return "checked", "The exact default auto-mode VPC includes the accepted region's default subnet. Existing project network and firewall policies still apply.", None
        await inspect("network", "Default VPC and subnet", base + "/global/networks/default", network_info)
        def absent(value, expected, kind, scope):
            if (value.get("kind") != kind or not _resource_matches(value.get("selfLink"), scope)
                    or value.get("nextPageToken")):
                raise ProbeUnavailable()
            warning = value.get("warning")
            if warning is not None and (not isinstance(warning, dict) or warning.get("code") != "NO_RESULTS_ON_PAGE"):
                raise ProbeUnavailable()
            rows = value.get("items", [])
            if not isinstance(rows, list) or len(rows) > 2 or any(not isinstance(v, dict) or v.get("name") != expected for v in rows):
                raise ProbeUnavailable()
            return ("blocked", "A resource with this package's name already exists. No existing resource is adopted or overwritten.", "Use the original installation's recovery receipt if it is yours, or choose a new name with the guide and rebuild.") if rows else ("checked", "A successful scoped collection read positively confirmed that this name is absent.", None)
        query = parse.urlencode({"filter": "name eq " + d["instance"], "maxResults": 2})
        await inspect("instance-collision", "New VM name", base + "/zones/" + zone + "/instances?" + query, lambda v: absent(v, d["instance"], "compute#instanceList", "/projects/" + project + "/zones/" + zone + "/instances"))
        rule = d["instance"] + "-iap-ssh"
        query = parse.urlencode({"filter": "name eq " + rule, "maxResults": 2})
        await inspect("firewall-collision", "New IAP firewall rule name", base + "/global/firewalls?" + query, lambda v: absent(v, rule, "compute#firewallList", "/projects/" + project + "/global/firewalls"))
        def metadata(value):
            if value.get("name") != project:
                raise ProbeUnavailable()
            items = value.get("commonInstanceMetadata", {}).get("items", [])
            if not isinstance(items, list) or any(not isinstance(v, dict) or not isinstance(v.get("key"), str) or not isinstance(v.get("value"), str) for v in items):
                raise ProbeUnavailable()
            enabled = [v["value"].lower() for v in items if v["key"] == "enable-oslogin"]
            if len(enabled) > 1 or any(v not in {"true", "false"} for v in enabled):
                raise ProbeUnavailable()
            return ("unknown", "This project enables OS Login. The supported preflight checks metadata SSH and cannot prove OS Login sudo access.", "Ask the implementation agent to support and verify the project's OS Login mode.") if enabled == ["true"] else ("checked", "Project metadata does not enable OS Login. Metadata SSH rights were checked separately; actual SSH and sudo access remain unverified.", None)
        info = await inspect("ssh-mode", "Project SSH mode", base, metadata)
        def quotas(value, *, global_scope=False):
            if value.get("name") != (project if global_scope else region):
                raise ProbeUnavailable()
            rows = value.get("quotas")
            if not isinstance(rows, list) or len(rows) > 256:
                raise ProbeUnavailable()
            by_name = {}
            for row in rows:
                if not isinstance(row, dict) or not isinstance(row.get("metric"), str) or row["metric"] in by_name:
                    raise ProbeUnavailable()
                limit, usage = row.get("limit"), row.get("usage")
                if (type(limit) not in {int, float} or type(usage) not in {int, float}
                        or not math.isfinite(limit) or not math.isfinite(usage) or limit < 0 or usage < 0):
                    raise ProbeUnavailable()
                by_name[row["metric"]] = limit - usage
            needed = {"FIREWALLS": 1} if global_scope else {"CPUS": shape["guestCpus"] if shape else None,
                "INSTANCES": 1, "SSD_TOTAL_GB": d["disk_gb"], "IN_USE_ADDRESSES": 1}
            if global_scope and "CPUS_ALL_REGIONS" in by_name:
                needed["CPUS_ALL_REGIONS"] = shape["guestCpus"] if shape else None
            if any(v is None or name not in by_name for name, v in needed.items()):
                return "unknown", "Required quota metrics or machine shape are missing from the observed scope; available quota cannot be inferred.", "Ask a project administrator to verify CPU, instance, disk, address and firewall quota."
            short = [name for name, quantity in needed.items() if by_name[name] < quantity]
            if short:
                if machine in {"e2-small", "e2-medium"} and all(name in {"CPUS", "CPUS_ALL_REGIONS"} for name in short):
                    return "unknown", "Observed CPU headroom is below the conservative guest-CPU check for a shared-core machine. This does not prove the actual shared-core quota request will fail.", "Verify the shared-core quota requirement with Google Cloud."
                return "blocked", "Observed quota headroom is below this fixed plan's conservative requirement: " + ", ".join(short) + ".", "Increase quota or revise the accepted machine/disk plan with the guide."
            return "checked", "Observed quota headroom covers this plan's conservative CPU, instance, disk, address or firewall checks. These values reserve nothing and do not prove live capacity.", None
        await inspect("regional-quota", "Regional quota headroom", base + "/regions/" + region, quotas)
        if info:
            try:
                status, detail, action = quotas(info, global_scope=True)
                checks.append(_check("project-quota", "Project quota headroom", status, detail, action=action))
            except (ProbeUnavailable, ValueError, TypeError, KeyError):
                checks.append(_check("project-quota", "Project quota headroom", "unknown", "Project firewall or global CPU quota could not be verified."))
        else:
            checks.append(_check("project-quota", "Project quota headroom", "unknown", "Project quota could not be verified from its project information response."))
        resource = "projects/" + project + "/zones/" + zone + "/instances/" + d["instance"]
        def policy(value, *, oslogin=False):
            expected = "projects/" + number + "/policies/compute." + ("requireOsLogin" if oslogin else "vmExternalIpAccess")
            if value.get("name") != expected:
                raise ProbeUnavailable()
            rules = value.get("spec", {}).get("rules")
            if not isinstance(rules, list) or len(rules) != 1 or not isinstance(rules[0], dict) or rules[0].get("condition"):
                return "unknown", "This effective policy is conditional or incomplete; the fixed plan's future resource cannot be proven allowed.", "Ask the project administrator to verify the accepted VM against its effective policy."
            row = rules[0]
            if oslogin:
                if type(row.get("enforce")) is not bool:
                    raise ProbeUnavailable()
                return ("unknown", "Effective policy requires OS Login. This adapter does not yet verify OS Login admin access needed for installation.", "Support and verify OS Login before preparing this deployment.") if row["enforce"] else ("checked", "The effective project policy does not require OS Login.", None)
            if row.get("allowAll") is True:
                return "checked", "The effective external-IP policy allows this plan's ephemeral external IPv4 address.", None
            if row.get("denyAll") is True:
                return "blocked", "Effective project policy denies external IPs, which this exact deploy script requests by default.", "Ask the implementation agent to support a VM without an external address, or select an allowed project."
            values = row.get("values")
            if not isinstance(values, dict):
                raise ProbeUnavailable()
            allow, deny = values.get("allowedValues"), values.get("deniedValues")
            if (allow is None) == (deny is None):
                raise ProbeUnavailable()
            selected = allow if allow is not None else deny
            if not isinstance(selected, list) or not selected or any(not isinstance(v, str) for v in selected):
                raise ProbeUnavailable()
            permitted = any(v.removeprefix("is:") == resource for v in selected)
            permitted = permitted if allow is not None else not permitted
            return ("checked", "The effective external-IP list policy permits this exact future VM resource.", None) if permitted else ("blocked", "The effective external-IP list policy does not permit this exact future VM resource.", "Revise the deployment plan or ask a policy administrator to review this VM.")
        for name, label, oslogin in (("vmExternalIpAccess", "Effective external IPv4 policy", False), ("requireOsLogin", "Effective OS Login policy", True)):
            await inspect("policy-" + name, label, "https://orgpolicy.googleapis.com/v2/projects/" + number + "/policies/compute." + name + ":getEffectivePolicy", lambda v, oslogin=oslogin: policy(v, oslogin=oslogin))
        checks.append(_check("live-capacity", "Actual provisioning and application readiness", "unknown", "No resources were created. VM capacity, actual mutation authorization, SSH/sudo, runtime bootstrap, custom UI and external channels remain unverified until their live operations succeed.", required=False))
        checks.append(_check("network-access", "Existing network access rules", "unknown", "The package creates a scoped IAP SSH rule, but existing default VPC and hierarchical firewall rules can allow broader SSH access. This preflight does not claim an IAP-only VM.", required=False))

    def _result(self, account, project, destination, checks):
        ready = bool(checks) and all(row["status"] == "checked" for row in checks if row["required"])
        plan = None
        if ready:
            # The original scripts choose core/account from environment, but
            # ordinary credential overrides have higher precedence. Remove the
            # supported environment override variables and preserve this point-
            # in-time configuration check rather than promising future identity.
            executable = getattr(self.command, "executable", None)
            command_path = (str(Path(executable).parent) + ":" if executable else "") + SAFE_PATH
            observed_home = self._context.get("HOME", str(Path.home()))
            observed_config = self._context.get("CLOUDSDK_CONFIG", str(Path(observed_home) / ".config/gcloud"))
            prefix = "env -i HOME=" + shlex.quote(observed_home) + " PATH=" + shlex.quote(command_path) + " CLOUDSDK_CONFIG=" + shlex.quote(observed_config) + " CLOUDSDK_CORE_ACCOUNT=" + shlex.quote(account) + " CLOUDSDK_CORE_PROJECT=" + shlex.quote(project) + " "
            plan = {"title": "Accepted package cloud deployment plan", "account": account,
                "destination": dict(destination),
                "resources": [f"New Debian 12 VM {destination['instance']} in {destination['zone']} using {destination['machine_type']}.",
                    f"{destination['disk_gb']} GB pd-balanced boot disk and an ephemeral external IPv4 address.",
                    "No VM service account or cloud API scopes.", f"Scoped IAP SSH firewall rule {destination['instance']}-iap-ssh on the existing default VPC."],
                "access": ["Application Console binds to VM loopback and is reached through a local IAP SSH tunnel.",
                    "Existing network/firewall rules still apply and can admit broader SSH access.",
                    "Private application credentials are entered directly in the VM terminal after provisioning; they are excluded from source upload."],
                "mutations": ["Executing the package deploy command may enable Compute Engine and IAP APIs, create billed VM/disk/firewall resources, upload accepted public application source and install prerequisites.",
                    "The setup command configures private application credentials and starts the owned background service on the VM.",
                    "Stopping/removing the application service does not delete the VM or stop its compute charges."],
                "commands": [prefix + "bash deploy/deploy-gcp.sh --dry-run", prefix + "bash deploy/deploy-gcp.sh",
                    prefix + "bash deploy/deploy-gcp.sh --setup", prefix + "bash deploy/tunnel-gcp.sh"],
                "limitations": ["Run these commands from the unchanged accepted package folder. The deploy script requires exact project confirmation before provisioning.",
                    "These commands pin the CLI location, home and configuration directory observed by this check. The selected account binding assumes that ordinary CLI configuration is still unchanged. Recheck after changing credentials, impersonation, CLI configuration or project settings.",
                    "Permissions and quotas are advisory observations; Google Cloud authorizes each actual operation. No capacity is reserved and no cloud deployment has been verified.",
                    "Keep the original package and its private deployment receipt for ownership-checked --resume after a partial installation."]}
        return {"account": account, "project": project, "checks": checks, "ready_for_plan": ready,
                "plan": plan, "provisioning_verified": False}
