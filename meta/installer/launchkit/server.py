"""Local guided setup, optional detailed editing, and deployment packaging."""

from __future__ import annotations

import asyncio
import hashlib
import logging
import platform
import shutil
import subprocess
import os
import re
from urllib.parse import quote
from contextlib import asynccontextmanager
from pathlib import Path

from fastapi import FastAPI, HTTPException, Path as ApiPath, Query, Request
from fastapi.exceptions import RequestValidationError
from fastapi.responses import FileResponse, JSONResponse, Response
from starlette.middleware.trustedhost import TrustedHostMiddleware
from starlette.formparsers import MultiPartException
from starlette.exceptions import HTTPException as StarletteHTTPException
from pydantic import BaseModel, ConfigDict, Field, model_serializer, model_validator

from .planner import design_team, planner_capability
from .schema import DeploymentSpec, DesignRequest
from .architect import ArchitectService, ArchitectUnavailable
from .projects import ProjectStore, DraftConflict, fingerprint
from .guide import GuideService
from .guide_store import GuideStore
from .organization import load_organization_plugin
from .guide_attachments import AttachmentReference, MAX_FILES, MAX_FILE_BYTES, MAX_TURN_BYTES
from .guide_operations import GuideOperations
from .guide_preview import staged_guide_preview
from .guide_timeline import GuideTimeline
from .visualization_operations import VisualizationConflict, VisualizationIntentConflict, VisualizationOperations
from .visualization_review import ReviewAdmissionUnavailable, VisualizationReviewService, bundled_review_reference_context
from .visual_review_runtime import VisualReviewRuntime
from .implementation import ImplementationService, BuildSelectionUnavailable
from .owner_shutdown import Owner, stop_owners
from .owned_console import (browser_console_url, console_response, resolve_owner,
                            resolve_guide_owner, MAX_BODY as CONSOLE_MAX_BODY)
from .forge_application_host import ForgeApplicationHost, application_setup_binding
from .local_application_registry import LocalApplicationRegistry
from .deployment import DeploymentService
from .installed_feedback import InstalledFeedbackNotAdmitted, InstalledSuccessNotAdmitted
from .remote_deployment import RemoteDeploymentService, RemoteProductActionNotSent
from .gcp_preflight import GcpPreflightService
from .gcp_provisioning import GcpProvisioningService
from .gcp_access import GcpAccessService
from .gcp_transport import GcpIapTransport
from .ssh_profiles import configured_ssh_transport
from .rehearsal import RehearsalService, StartNotAdmitted, start_request_fingerprint
from .provider_settings import ProviderSettings, ProviderSettingsUnavailable, canonical_setup_provider
from .checker_setup import CheckerSetupService
from .toolkit_saved_requests import ToolkitSavedRequests, ToolkitSavedRequestsError, MAX_MANIFEST_BYTES

APP_ROOT = Path(__file__).resolve().parents[1]
checker_setup = CheckerSetupService(APP_ROOT)
LOG = logging.getLogger(__name__)


def setup_home():
    """Reuse saved setup state with Forge settings or their existing aliases."""
    home = os.environ.get("META_FORGE_HOME") or os.environ.get("MOBKIT_LAUNCH_HOME", str(APP_ROOT / ".studio"))
    return Path(home).expanduser().resolve()


STUDIO_ROOT = setup_home()
provider_settings = ProviderSettings(STUDIO_ROOT)
architect = ArchitectService(STUDIO_ROOT)
architect.provider_settings = provider_settings
projects = ProjectStore(STUDIO_ROOT)
guide = GuideService(GuideStore(STUDIO_ROOT), architect, organization=load_organization_plugin())
toolkit_saved_requests = ToolkitSavedRequests(STUDIO_ROOT / 'toolkit-saved-requests')


def saved_toolkit_review_evidence(guide_id):
    retained = toolkit_saved_requests.status(guide_id)
    review, selection = retained.get('review'), retained.get('selection')
    if (retained.get('status') != 'review_required' or retained.get('audience_approved') is not True
            or retained.get('guide_id') != guide_id or not isinstance(review, dict) or not isinstance(selection, dict)
            or review.get('scope') != 'successor_configuration_candidate'
            or review.get('status') != 'review_required'
            or review.get('prior_request_sha256') != selection.get('request_sha256')
            or not re.fullmatch(r'[0-9a-f]{64}', selection.get('request_sha256', ''))
            or type(selection.get('version')) is not int or selection['version'] < 1):
        return None
    return {'scope': 'saved_configuration_proposal_only', 'guide_id': guide_id,
            'selection': retained['selection'], 'review': retained['review'],
            'current_operations': [], 'installed': False,
            'detail': 'Host-approved saved settings reviewed at the recorded guide version against Toolkit help. '
                      'Historical operations are unselected. Installation, account access and agent grants remain separate.'}


architect.saved_toolkit_review_provider = saved_toolkit_review_evidence
guide_operations = GuideOperations()
guide_timeline = GuideTimeline(guide.store, architect)
visualization_operations = VisualizationOperations(root=STUDIO_ROOT / 'visualizations' / 'operations')
REVIEW_RENDERER_HASH = hashlib.sha256((APP_ROOT / 'web/freeform-animation.mjs').read_bytes()).hexdigest()
visual_review_runtime = VisualReviewRuntime(STUDIO_ROOT / 'visual-review-runtime-v1')
visualization_reviews = VisualizationReviewService(runtime=visual_review_runtime.get_runtime,
    operations=visualization_operations, root=STUDIO_ROOT / 'visualizations' / 'reviews',
    expected_renderer_hash=REVIEW_RENDERER_HASH, runtime_owner_id=visual_review_runtime.runtime_owner_id,
    reference_context=bundled_review_reference_context())
visual_review_runtime.reviews = visualization_reviews
implementation = ImplementationService(STUDIO_ROOT, guide, guide_operations)
application_host = None


def selected_package_runtime(state):
    """Project the same protected target offered to the real Build owner."""
    if application_host is None or not (state.get("proposal") or state.get("blueprint")):
        return None
    if type(application_host) is LocalApplicationRegistry:
        # This is support metadata, including while an interview is running.
        # Completed guide/version admission still belongs to deliberate Build.
        return application_host._runtime_target_for_blueprint(state.get("proposal") or state["blueprint"])
    else:
        options = application_host.initial_build_options(state["id"])
    return options.get("runtime_target")



guide.package_runtime_provider = selected_package_runtime


def select_application_host(selected):
    """Existing trusted installation owner injection, never an HTTP selector."""
    global application_host
    if type(selected) is not ForgeApplicationHost:
        raise TypeError("Select the original trusted Forge application host.")
    if application_host is not None and application_host is not selected:
        raise DraftConflict("Preserve the original selected application host.")
    selected.require_current()
    if (application_host is None and
            getattr(implementation.prepare_workspace, "__func__", None) is not ImplementationService.prepare_workspace):
        raise DraftConflict("Preserve the existing workspace source owner.")
    if application_host is None and deployment.application_setup_selection is not None:
        raise DraftConflict("Preserve the existing application startup owner.")
    if application_host is None and getattr(deployment, "application_setup_preparation", None) is not None:
        raise DraftConflict("Preserve the existing application preparation owner.")
    if application_host is None and getattr(deployment, "application_setup_retention", None) is not None:
        raise DraftConflict("Preserve the existing Install completion owner.")
    continuation = selected.continue_application_startup if selected._startup_continuation is not None else None
    if application_host is None and getattr(deployment, "application_setup_continuation", None) is not None:
        raise DraftConflict("Preserve the existing application continuation owner.")
    replacement = selected._selected_replacement if selected._replacement_selection is not None else None
    if application_host is None and getattr(deployment, "application_replacement_selection", None) is not None:
        raise DraftConflict("Preserve the existing installed replacement owner.")
    if getattr(deployment, "application_host_selection", None) is not None:
        raise DraftConflict("Preserve the existing per-installation application owner.")
    if application_host is selected and (
            implementation.prepare_workspace != selected.prepare_workspace
            or deployment.application_setup_selection != selected._selected_startup
            or getattr(deployment, "application_setup_preparation", None) != selected.prepare_application
            or getattr(deployment, "application_setup_retention", None) != selected.retain_installation_inputs
            or getattr(deployment, "application_setup_continuation", None) != continuation
            or getattr(deployment, "application_replacement_selection", None) != replacement):
        raise DraftConflict("The selected application's original callbacks changed.")
    implementation.prepare_workspace = selected.prepare_workspace
    deployment.application_setup_selection = selected._selected_startup
    deployment.application_setup_preparation = selected.prepare_application
    deployment.application_setup_retention = selected.retain_installation_inputs
    deployment.application_setup_continuation = continuation
    deployment.application_replacement_selection = replacement
    application_host = selected


def select_local_application_runtime(selected, *, remote_control_selection=None, ordinary_meta_gateway=None,
        model_configuration=None, replacement_selection=None, retained_ordinary_applications=(),
        application_configuration_selection=None):
    """Trusted launcher selection shared by automatic per-user Build owners.

    application_configuration_selection(guide_id, guide_version) is an optional
    synchronous original-owner reader returning ExactFile or None. Its complete
    declaration document includes the Console fields, never credential values.
    Fresh Builds select it once; continuations retain their original source.
    """
    global application_host
    from .runtime_selection import MetaToolkitRuntimeTarget
    if type(selected) is not MetaToolkitRuntimeTarget:
        raise TypeError("Select the original trusted Meta Toolkit runtime target.")
    if remote_control_selection is not None:
        from .remote_product_control import ProductControlSelection
        if type(remote_control_selection) is not ProductControlSelection:
            raise TypeError("Select the original private Linux product control owner.")
    from .connect_application_startup import _ModelConfiguration
    if model_configuration is not None and type(model_configuration) is not _ModelConfiguration:
        raise TypeError("Select the original application model configuration owner.")
    if model_configuration is not None:
        model_configuration.require_environment()
        model_configuration.require_filesystem_current()
    selected_models = None if model_configuration is None else model_configuration.selection()
    callbacks = (("application_setup_selection", "_selected_startup"),
        ("application_setup_preparation", "prepare_application"),
        ("application_setup_retention", "retain_installation_inputs"),
        ("application_setup_continuation", "continue_application_startup"),
        ("application_replacement_selection", "_selected_replacement"),
        ("application_host_selection", "selected_host"))
    if application_host is not None:
        if (type(application_host) is not LocalApplicationRegistry
                or application_host.runtime_target is not selected
                or application_host.remote_control_selection is not remote_control_selection
                or application_host.replacement_selection is not replacement_selection
                or application_host.application_configuration_selection is not application_configuration_selection
                or application_host.retained_ordinary_applications is not retained_ordinary_applications
                or application_host.ordinary_meta_gateway is not ordinary_meta_gateway):
            raise DraftConflict("Preserve the original selected application runtime owner.")
        application_host._require_registry()
        if application_host._selected_model_configuration() != selected_models:
            raise DraftConflict("Preserve the original application model configuration selection.")
        if (implementation.prepare_workspace != application_host.prepare_workspace
                or any(getattr(deployment, name, None) != getattr(application_host, method)
                    for name, method in callbacks)):
            raise DraftConflict("The original application registry callbacks changed.")
        return application_host
    if (getattr(implementation.prepare_workspace, "__func__", None) is not ImplementationService.prepare_workspace
            or any(getattr(deployment, name, None) is not None for name, _ in callbacks)):
        raise DraftConflict("Preserve the existing application source and installation owners.")
    # Application credentials belong to its original trusted launcher. The
    # guide's runtime and provider configuration are independent consumers.
    registry = LocalApplicationRegistry(guide_store=guide.store, implementation=implementation,
        deployment=deployment, runtime_target=selected, remote_control_selection=remote_control_selection,
        model_configuration=selected_models, ordinary_meta_gateway=ordinary_meta_gateway,
        replacement_selection=replacement_selection, retained_ordinary_applications=retained_ordinary_applications,
        application_configuration_selection=application_configuration_selection)
    if model_configuration is not None:
        model_configuration.require_environment()
        model_configuration.require_filesystem_current()
        if model_configuration.selection() != selected_models:
            raise DraftConflict("The original application model configuration changed during selection.")
    implementation.prepare_workspace = registry.prepare_workspace
    for name, method in callbacks:
        setattr(deployment, name, getattr(registry, method))
    application_host = registry
    return registry

def select_product_application_runtime(selected):
    """Bind one source-owned local/remote selection before serving requests."""
    from .application_host_launch import ApplicationRuntimeSelection
    if type(selected) is not ApplicationRuntimeSelection:
        raise TypeError("Select the original trusted local and remote runtime owners.")
    control = selected.remote_control
    if (remote_deployment.product_control_selection is not remote_deployment._product_control_selection
            or remote_deployment.product_control_selection not in (None, control)):
        raise DraftConflict("Preserve the original selected remote control owner.")
    control.require_current()
    if (remote_deployment.product_control_selection is not remote_deployment._product_control_selection
            or remote_deployment.product_control_selection not in (None, control)):
        raise DraftConflict("Preserve the original selected remote control owner.")
    registry = select_local_application_runtime(selected.local_target, remote_control_selection=control)
    remote_deployment.product_control_selection = remote_deployment._product_control_selection = control
    return registry


def _selected_remote_product_owner():
    """Return trusted selection data; actual product admission checks its owner."""
    selected = remote_deployment.product_control_selection
    if selected is None and remote_deployment._product_control_selection is None:
        return None
    from .remote_product_control import ProductControlSelection
    if (type(selected) is not ProductControlSelection
            or selected is not remote_deployment._product_control_selection
            or type(application_host) is not LocalApplicationRegistry
            or application_host.remote_control_selection is not selected):
        raise DraftConflict("Preserve the original selected remote control owner.")
    application_host._require_registry()
    return selected


implementation.checker_setup = checker_setup
guide.implementation_evidence_provider = lambda state: implementation.status(state["id"], state=state)
deployment = DeploymentService(STUDIO_ROOT, guide, implementation, guide_operations)
gcp_preflight = GcpPreflightService(deployment)
gcp_access = GcpAccessService(STUDIO_ROOT, gcp_preflight)
gcp_provisioning = GcpProvisioningService(STUDIO_ROOT, gcp_preflight)
gcp_access.provisioning = gcp_provisioning
gcp_provisioning.prepare_access = lambda value: gcp_access.prepare_access(
    value["binding"], value["destination"], value["operation_id"])
cloud_transport = None
if gcp_preflight.command.available:
    cloud_home = gcp_preflight.command.environment.get("HOME", str(Path.home()))
    cloud_config = gcp_preflight.command.environment.get("CLOUDSDK_CONFIG", str(Path(cloud_home) / ".config/gcloud"))
    cloud_transport = GcpIapTransport(gcp_provisioning.verify, gcp_access.host_keys, gcp_access.identity_file,
        gcloud_executable=gcp_preflight.command.executable, cli_home=cloud_home, cli_config_directory=cloud_config)
remote_deployment = RemoteDeploymentService(STUDIO_ROOT, guide, implementation, deployment,
    transport=configured_ssh_transport(), cloud_transport=cloud_transport)
remote_deployment.cloud_removal_status = gcp_provisioning.lifecycle
gcp_provisioning.prepare_removal = remote_deployment.prepare_cloud_removal
guide.deployment_evidence_provider = lambda state: deployment.guide_status(state["id"], state=state)
guide.remote_deployment_evidence_provider = lambda state: remote_deployment.guide_status(state["id"], state=state)
rehearsal = RehearsalService(STUDIO_ROOT, guide, guide_operations, implementation=implementation)
guide.trial_evidence_provider = lambda state: rehearsal.status(state["id"], state=state)


@asynccontextmanager
async def lifespan(app):
    try:
        provider_settings.restore()
        # Reconcile only persisted positive native termination observations.
        # An invalid critic proof disables that feature without blocking setup.
        await visual_review_runtime.reconcile_terminated()
        yield
    finally:
        # Each service retains ownership of different native work. A failed or
        # hung cleanup must not prevent the remaining owners from stopping theirs.
        # The deployment, implementation and rehearsal workers stop themselves
        # on owner loss; stop_owners never signals them, and no owner names
        # its groups.
        def workers(service):
            return lambda: list((getattr(service, "processes", None) or {}).values())
        failures = await stop_owners((
            Owner("guide operations", guide_operations),
            Owner("visualization operations", visualization_operations),
            Owner("visualization reviews", visualization_reviews),
            Owner("visual critic runtime", visual_review_runtime),
            Owner("remote deployment", remote_deployment),
            Owner("GCP resource preparation", gcp_provisioning),
            Owner("GCP access checks", gcp_preflight),
            Owner("deployment", deployment, workers=workers(deployment)),
            Owner("rehearsal", rehearsal, workers=workers(rehearsal)),
            Owner("implementation", implementation, workers=workers(implementation)),
            Owner("checker preparation", checker_setup),
            Owner("saved Toolkit requests", toolkit_saved_requests),
            Owner("architect", architect),
        ))
        if failures:
            raise BaseExceptionGroup(
                "Meta Forge cleanup failed for: " + ", ".join(name for name, _ in failures),
                [error for _, error in failures],
            )


app = FastAPI(title="Meta Forge", docs_url=None, redoc_url=None, lifespan=lifespan)
app.add_middleware(TrustedHostMiddleware, allowed_hosts=["127.0.0.1", "localhost", "[::1]", "testserver"])
design_lock = asyncio.Lock()


@app.exception_handler(RequestValidationError)
async def request_validation_error(request: Request, error: RequestValidationError):
    # Installation fields and trial feedback may contain private context. Return
    # the field and reason, never rejected input or embedded exception context.
    if "/deployment" in request.url.path or "/remote-deployment" in request.url.path or "/gcp-preflight" in request.url.path or "/gcp-deployment" in request.url.path or "/rehearsal" in request.url.path or "/toolkit-saved" in request.url.path or request.url.path.startswith('/api/visualizations') or request.url.path == '/api/provider-settings':
        def private_location(location):
            if '/toolkit-saved' in request.url.path:
                public = {'body', 'query', 'path', 'identity', 'version', 'manifest_sha256',
                          'request_id', 'request_sha256', 'host_fingerprint', 'audience_approved'}
                return tuple(part if type(part) is int or part in public else 'private_field' for part in location)
            if request.url.path == '/api/provider-settings':
                public = {'body', 'provider', 'api_key'}
                return tuple(part if type(part) is int or part in public else 'private_field' for part in location)
            if "/rehearsal/" in request.url.path and request.url.path.endswith("/replay"):
                public = {"body", "query", "path", "identity", "rehearsal_id", "version",
                          "original_build_id", "original_source_digest", "build_id", "source_digest", "start_request_id"}
                return tuple(part for part in location if type(part) is int or part in public)
            if request.url.path.endswith("/use/success"):
                public = {"body", "query", "path", "identity", "version", "deployment_id",
                          "build_id", "source_digest", "package_sha256", "request_id", "task"}
                return tuple(part for part in location if type(part) is int or part in public)
            if "settings" in location:
                index = location.index("settings")
                # Dictionary keys are submitted private input too. Retain the
                # public field and any array indices without echoing keys.
                return (*location[:index + 1], *(part for part in location[index + 1:] if type(part) is int))
            if "/remote-deployment" in request.url.path:
                public = {"body", "query", "path", "identity", "version", "build_id", "deployment_id",
                          "host", "user", "ssh_port", "destination", "action", "force", "entry_id", "access_id"}
                return tuple(part for part in location if type(part) is int or part in public)
            if "/gcp-preflight" in request.url.path or "/gcp-deployment" in request.url.path:
                public = {"body", "query", "path", "identity", "version", "build_id", "account", "project", "request_id", "operation_id", "acknowledge_data_loss"}
                return tuple(part for part in location if type(part) is int or part in public)
            return location
        return JSONResponse({"detail": [{"loc": private_location(item["loc"]), "type": item["type"],
                                         "msg": item["msg"]} for item in error.errors()]}, status_code=422)
    from fastapi.exception_handlers import request_validation_exception_handler
    return await request_validation_exception_handler(request, error)


@app.middleware("http")
async def local_requests(request: Request, call_next):
    origin = request.headers.get("origin")
    allowed = {f"{request.url.scheme}://{request.url.netloc}", "http://127.0.0.1:5173", "http://localhost:5173"}
    if request.url.path.startswith("/api/") and origin and origin not in allowed:
        return JSONResponse({"detail": "Open Meta Forge from its local setup URL."}, status_code=403)
    if request.method in ("POST", "PUT"):
        # The animation request includes the fully resolved graph, not just its
        # class definitions. Keep every identity and relationship for large mobs.
        if request.url.path.startswith("/api/owned-console/"):
            limit = CONSOLE_MAX_BODY
        elif request.url.path == "/api/visualizations":
            limit = 8_000_000
        elif re.fullmatch(r'/api/visualizations/[0-9a-f]{64}/review', request.url.path):
            limit = 20_000_000
        elif request.method == "POST" and re.fullmatch(
                r'/api/guide/[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}/attachments', request.url.path):
            # The upload handler enforces file/turn limits. Allow bounded
            # multipart headers without raising limits for other guide input.
            limit = MAX_TURN_BYTES + 64 * 1024
        else:
            limit = 1_000_000 if request.url.path.startswith(("/api/blueprint", "/api/architect", "/api/projects", "/api/connectors", "/api/guide")) else 100_000
        try:
            length = int(request.headers.get("content-length", "0"))
        except ValueError:
            return JSONResponse({"detail": "Invalid request length."}, status_code=400)
        if length > limit:
            return JSONResponse({"detail": "This configuration exceeds the request size limit."}, status_code=413)
        body = bytearray()
        async for chunk in request.stream():
            body.extend(chunk)
            if len(body) > limit:
                return JSONResponse({"detail": "This setup is too large."}, status_code=413)
        request._body = bytes(body)
    response = await call_next(request)
    response.headers["X-Content-Type-Options"] = "nosniff"
    response.headers["Referrer-Policy"] = "no-referrer"
    response.headers["Cache-Control"] = "no-store" if request.url.path.startswith("/api/") else "no-cache"
    return response


def cloud_capability() -> dict:
    binary = shutil.which("gcloud")
    if not binary:
        return {"available": False, "project": None, "reason": "The package includes Google Cloud CLI setup instructions."}
    try:
        result = subprocess.run([binary, "config", "get-value", "project"], capture_output=True, text=True, timeout=5, check=False)
        value = result.stdout.strip()
        project = value if result.returncode == 0 and value and value != "(unset)" else None
    except (OSError, subprocess.TimeoutExpired):
        project = None
    return {"available": True, "project": project, "reason": "Google Cloud CLI found. Check the selected account and project in guided deployment before provisioning."}


def forge_release():
    """The Forge release: installer/pyproject.toml's version, which forge-release.py tags."""
    import tomllib
    return tomllib.loads((APP_ROOT / "pyproject.toml").read_text())["project"]["version"]


def ordinary_runtime():
    """The Meta runtime and gateway the trusted launcher selected for ordinary
    applications; None without such a selection, as in the developer entry."""
    host = application_host
    if type(host) is not LocalApplicationRegistry or host.runtime_target is None or host.ordinary_meta_gateway is None:
        return None
    target = host.runtime_target
    return {"sdk": target.meta.distribution, "version": target.meta.version, "target": target.target,
            "gateway_sha256": host.ordinary_meta_gateway.sha256}


def agent_team_runtime():
    """The public Meta release every compiled agent team installs, in every entry."""
    from . import public_meta_runtime
    return {"sdk": public_meta_runtime.SDK_DISTRIBUTION, "version": public_meta_runtime.VERSION,
            "release": public_meta_runtime.RELEASE_URL}


@app.get("/api/capabilities")
async def capabilities():
    return {"planner": planner_capability(), "platform": "mac" if platform.system() == "Darwin" else "linux", "gcp": await asyncio.to_thread(cloud_capability),
            "version": forge_release(), "ordinary_runtime": ordinary_runtime(), "agent_team_runtime": agent_team_runtime(),
            "rehearsal": {"start_request_replay": True}, "agent_team": deployment.agent_team_availability()}


class BlueprintBody(BaseModel):
    model_config = ConfigDict(extra="forbid")
    blueprint: dict


class VisualizationParent(BaseModel):
    model_config = ConfigDict(extra="forbid")
    operation_id: str = Field(pattern=r'^[0-9a-f]{64}$', strict=True)
    code_hash: str = Field(pattern=r'^[0-9a-f]{64}$', strict=True)
    blueprint_hash: str = Field(pattern=r'^[0-9a-f]{64}$', strict=True)
    model_hash: str = Field(pattern=r'^[0-9a-f]{64}$', strict=True)
    setup_id: str | None = Field(min_length=1, max_length=256, strict=True)
    stream_id: str = Field(min_length=1, max_length=256, strict=True)


class VisualizationConsumerDeclaration(BaseModel):
    model_config = ConfigDict(extra="forbid")
    consumer_id: str = Field(pattern=r'^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$', strict=True)
    intent_revision: int = Field(gt=0, le=9007199254740991, strict=True)
    setup_id: str | None = Field(default=None, min_length=1, max_length=256, pattern=r".*\S.*", strict=True)
    stream_id: str = Field(min_length=1, max_length=256, pattern=r".*\S.*", strict=True)


class VisualizationConsumerIntent(VisualizationConsumerDeclaration):
    intent_token: str = Field(min_length=1, max_length=256, strict=True)


class VisualizationConsumerRelease(VisualizationConsumerDeclaration):
    intent_token: str | None = Field(default=None, min_length=1, max_length=256, strict=True)


async def visualization_intent_response(action, value):
    try:
        return await action(**value.model_dump())
    except VisualizationIntentConflict:
        return JSONResponse(status_code=409, content={"code": "stale_visualization_intent",
            "detail": "This view no longer requests that animation. Check the current proposal again."})
    except ValueError:
        raise HTTPException(422, "The animation view intent could not be validated.") from None


@app.post('/api/visualization-intents/declare')
async def visualization_intent_declare(value: VisualizationConsumerDeclaration):
    return await visualization_intent_response(visualization_operations.declare_intent, value)


@app.post('/api/visualization-intents/release')
async def visualization_intent_release(value: VisualizationConsumerRelease):
    return await visualization_intent_response(visualization_operations.release_intent, value)


@app.post('/api/visualization-intents/renew')
async def visualization_intent_renew(value: VisualizationConsumerIntent):
    return await visualization_intent_response(visualization_operations.renew_intent, value)


class VisualizationBody(BlueprintBody):
    model: dict
    feedback: str = Field(default="", max_length=1000)
    stream_id: str | None = Field(default=None, min_length=1, max_length=256, pattern=r".*\S.*", strict=True)
    setup_id: str | None = Field(default=None, min_length=1, max_length=256, pattern=r".*\S.*", strict=True)
    expected_parent: VisualizationParent | None = None
    replay_parent: VisualizationParent | None = None
    review_id: str | None = Field(default=None, pattern=r'^[0-9a-f]{64}$', strict=True)
    consumer_intent: VisualizationConsumerIntent | None = None


@app.post("/api/visualizations", status_code=202)
async def visualization_generate(value: VisualizationBody):
    from .visualizer import checked_inputs
    try:
        blueprint, model = checked_inputs(value.blueprint, value.model)
    except ValueError:
        raise HTTPException(422, "The proposal graph could not be validated.") from None
    correction = {}
    if value.replay_parent is not None:
        if value.expected_parent is not None or value.review_id is not None:
            raise HTTPException(409, 'Parent restoration cannot request a new visual correction.')
        correction = {'replay_parent': value.replay_parent.model_dump()}
    if value.expected_parent is not None or value.review_id is not None:
        try:
            parent = value.expected_parent.model_dump() if value.expected_parent is not None else None
            review = visualization_reviews.get(value.review_id) if value.review_id is not None else None
            advisory = (review or {}).get('correction_advisory') or {}
            if (parent is None or not review or review.get('status') != 'reviewed'
                    or advisory.get('expected_parent') != parent or advisory.get('review_id') != value.review_id
                    or advisory.get('feedback') != value.feedback):
                raise ValueError('The review does not authorize this correction.')
            correction = {'expected_parent': parent}
        except (ValueError, KeyError, TypeError):
            raise HTTPException(409, 'This correction does not match its saved visual review.') from None
    try:
        return await visualization_operations.submit(architect.visualizer, blueprint, model, feedback=value.feedback,
            **({'stream_id': value.stream_id} if value.stream_id is not None else {}),
            **({'setup_id': value.setup_id} if value.setup_id is not None else {}),
            **({'consumer_intent': value.consumer_intent.model_dump()} if value.consumer_intent is not None else {}), **correction)
    except VisualizationIntentConflict:
        return JSONResponse(status_code=409, content={"code": "stale_visualization_intent",
            "detail": "This view no longer requests that animation. Check the current proposal again."})
    except VisualizationConflict:
        if value.replay_parent is not None:
            raise HTTPException(409, 'The saved animation could not be restored exactly. No new work was sent.') from None
        raise HTTPException(409, 'A newer scene request superseded this visual correction.') from None


def visual_review_capability():
    return {**visual_review_runtime.capability(), 'renderer_hash': REVIEW_RENDERER_HASH}


@app.get('/api/visualization-review-capability')
async def visualization_review_capability():
    return visual_review_capability()


class VisualizationReviewBody(BlueprintBody):
    model: dict
    binding: dict
    frames: list[dict] = Field(min_length=1, max_length=8)
    renderer_hash: str = Field(pattern=r'^[0-9a-f]{64}$', strict=True)


@app.post('/api/visualizations/{identity}/review', status_code=202)
async def visualization_review_submit(identity: str, value: VisualizationReviewBody):
    if not visual_review_capability()['available']:
        raise HTTPException(503, 'The visual critic is not active. The working animation is preserved.')
    try:
        if value.binding.get('id') != identity:
            raise ValueError('The review operation differs from its route.')
        return await visualization_reviews.submit(binding=value.binding, blueprint=value.blueprint,
            model=value.model, frames=value.frames, renderer_hash=value.renderer_hash)
    except ReviewAdmissionUnavailable:
        raise HTTPException(429, 'The available visual review attempts are in use. The working animation is preserved.') from None
    except (ValueError, KeyError, TypeError):
        raise HTTPException(409, 'The captured review does not match this published proposal and renderer.') from None


@app.get('/api/visualizations/{identity}/reviews/{review_id}')
async def visualization_review_status(identity: str, review_id: str):
    try:
        review = visualization_reviews.get(review_id)
    except (ValueError, KeyError, TypeError, OSError):
        review = None
    if review is None or review.get('binding', {}).get('id') != identity:
        raise HTTPException(404, 'This visual review is no longer available.')
    return review


@app.get('/api/visualizations/{identity}/timeline')
async def visualization_timeline(identity: str,
        request_fingerprint: str = Query(pattern=r'^[0-9a-f]{64}$'),
        blueprint_hash: str = Query(pattern=r'^[0-9a-f]{64}$'),
        model_hash: str = Query(pattern=r'^[0-9a-f]{64}$'),
        native_request_key: str = Query(pattern=r'^[0-9a-f]{64}$'),
        source_generation: int = Query(ge=1),
        stream_id: str | None = Query(default=None, min_length=1, max_length=256),
        setup_id: str | None = Query(default=None, min_length=1, max_length=256),
        after: str | None = Query(default=None, pattern=r'^console:[0-9]+$', max_length=40),
        limit: int = Query(default=100, ge=1, le=200)):
    from .visualization_timeline import read_activity
    try:
        return await read_activity(visualization_operations, architect.visualizer, identity,
            {'request_fingerprint': request_fingerprint, 'blueprint_hash': blueprint_hash,
             'model_hash': model_hash, 'native_request_key': native_request_key,
             'source_generation': source_generation,
             'stream_id': stream_id, 'setup_id': setup_id}, after=after, limit=limit)
    except LookupError:
        raise HTTPException(404, 'This scene request is no longer available.') from None
    except ValueError:
        raise HTTPException(409, 'The scene activity does not match this proposal and request. Refresh the preview to reconnect.') from None


@app.get('/api/visualizations/published/latest')
async def visualization_latest_saved(
        setup_id: str = Query(min_length=1, max_length=256),
        stream_id: str = Query(min_length=1, max_length=256),
        blueprint_hash: str = Query(pattern=r'^[0-9a-f]{64}$'),
        model_hash: str = Query(pattern=r'^[0-9a-f]{64}$')):
    try:
        receipt = visualization_operations.latest_saved(architect.visualizer, setup_id=setup_id,
            stream_id=stream_id, blueprint_hash=blueprint_hash, model_hash=model_hash)
    except ValueError:
        raise HTTPException(422, 'The saved animation scope could not be validated.') from None
    if receipt is None:
        raise HTTPException(404, 'No saved animation matches this proposal and view.')
    return receipt


class SavedVisualizationSelection(BaseModel):
    model_config = ConfigDict(extra='forbid')
    publication: VisualizationParent
    consumer_intent: VisualizationConsumerIntent
    require_latest: bool = Field(default=True, strict=True)


@app.post('/api/visualizations/published/adopt')
async def visualization_adopt_saved(value: SavedVisualizationSelection):
    try:
        return await visualization_operations.adopt_saved(architect.visualizer,
            publication=value.publication.model_dump(), consumer_intent=value.consumer_intent.model_dump(),
            require_latest=value.require_latest)
    except VisualizationIntentConflict:
        return JSONResponse(status_code=409, content={'code': 'stale_visualization_intent',
            'detail': 'This view no longer requests that animation. Check the current proposal again.'})
    except VisualizationConflict:
        raise HTTPException(409, 'The saved animation changed or no longer matches this proposal. Check it again.') from None
    except ValueError:
        raise HTTPException(422, 'The saved animation selection could not be validated.') from None


@app.get("/api/visualizations/{identity}")
async def visualization_status(identity: str):
    receipt = visualization_operations.get(identity)
    if receipt is None:
        raise HTTPException(404, "This scene request is no longer active. Refresh the preview to reconnect.")
    return receipt


class ArchitectMessage(BlueprintBody):
    message: str = Field(min_length=1, max_length=12000)


class ProjectSave(BlueprintBody):
    expected_revision: int = Field(ge=0, strict=True)


class InspectConnection(BaseModel):
    model_config = ConfigDict(extra="forbid")
    connection: dict


class ManualIntent(BaseModel):
    model_config = ConfigDict(extra="forbid")
    base: dict
    draft: dict


class GuideResponse(BaseModel):
    model_config = ConfigDict(extra="forbid")
    version: int = Field(ge=1, strict=True)
    action: str = Field(default="answer", pattern="^(answer|chat|back|manual|organization)$")
    organization_fingerprint: str | None = Field(default=None, strict=True, pattern=r"^[0-9a-f]{64}$")
    selected: list[str] = Field(default_factory=list, max_length=6)
    text: str = Field(default="", max_length=6000)
    attachments: list[AttachmentReference] = Field(default_factory=list, max_length=MAX_FILES)
    stage: str | None = None
    blueprint: dict | None = None
    manual_intent: ManualIntent | None = None
    combine_options: bool = Field(default=False, strict=True)

    @model_validator(mode="after")
    def combined_answer(self):
        if self.action == "organization":
            if (self.organization_fingerprint is None or self.selected or self.text or self.attachments
                    or self.stage is not None or self.blueprint is not None
                    or self.manual_intent is not None or self.combine_options):
                raise ValueError("Select only the displayed workplace guidance for this setup.")
        elif self.organization_fingerprint is not None:
            raise ValueError("Select workplace guidance with its dedicated action.")
        if self.manual_intent is not None and (self.action != "manual" or self.blueprint is not None):
            raise ValueError("Review manual intent without replacing the current blueprint.")
        if self.attachments and self.action not in {"answer", "chat"}:
            raise ValueError("Attach evidence to an answer or chat message.")
        if self.combine_options and (self.action != "answer" or len(self.selected) < 2):
            raise ValueError("Combine at least two offered options as an answer to the current question.")
        return self

    @model_serializer(mode="wrap")
    def request_body(self, handler):
        body = handler(self)
        # Ordinary answers retain their saved request identity across upgrades.
        if self.combine_options is False:
            body.pop("combine_options", None)
        if not self.attachments:
            body.pop("attachments", None)
        if self.manual_intent is None:
            body.pop("manual_intent", None)
        if self.organization_fingerprint is None:
            body.pop("organization_fingerprint", None)
        return body


class GuideStart(BaseModel):
    model_config = ConfigDict(extra="forbid")
    start_request_id: str | None = Field(default=None, strict=True,
        pattern=r"^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$")


class GuideApply(BaseModel):
    model_config = ConfigDict(extra="forbid")
    version: int = Field(ge=1, strict=True)


async def guide_result(operation, *, failure_detail=None, timeout_detail=None):
    try:
        return await operation
    except (InstalledFeedbackNotAdmitted, InstalledSuccessNotAdmitted) as error:
        return JSONResponse({"detail": str(error), "code": error.code}, status_code=error.status_code)
    except LookupError as error:
        raise HTTPException(404, str(error)) from None
    except BuildSelectionUnavailable as error:
        return JSONResponse({"detail": str(error), "code": "build_selection_unavailable"}, status_code=409)
    except RemoteProductActionNotSent as error:
        return JSONResponse({"detail": str(error), "code": error.code,
            "action": error.action, "action_request_id": error.action_request_id}, status_code=409)
    except DraftConflict as error:
        raise HTTPException(409, str(error)) from None
    except ArchitectUnavailable as error:
        raise HTTPException(503, str(error)) from None
    except ValueError as error:
        raise HTTPException(422, str(error)) from None
    except TimeoutError:
        raise HTTPException(504, timeout_detail or "The setup agent is still working. Open its console for progress before retrying.") from None
    except Exception as error:
        LOG.warning("Guided setup failed (%s)", type(error).__name__)
        raise HTTPException(502, failure_detail or "The setup agent could not finish. Your last saved step is preserved. Retry or inspect its native console.") from None


@app.get("/api/guide/new")
async def new_guide():
    return {**guide.new(), "input_surface": guide.input_surface()}


@app.get("/api/guide/list")
async def list_guides():
    return {"setups": await asyncio.to_thread(guide.store.list)}


@app.post("/api/guide/start")
async def start_guide(value: GuideStart | None = None):
    state = await guide_result(guide.create(value.start_request_id if value else None))
    return {**state, "input_surface": guide.input_surface()}


@app.get("/api/guide/creation/{request_id}")
async def read_guide_creation(request_id: str):
    created = await guide_result(asyncio.to_thread(guide.store.created_request, request_id))
    if created is None:
        raise HTTPException(404, "This setup creation is not yet confirmed. Check again before starting another setup.")
    return await read_guide(created["id"])


@app.get("/api/guide/{identity}")
async def read_guide(identity: str):
    state = await guide_result(guide_operations.read(guide, identity))
    state["input_surface"] = guide.input_surface()
    preview = staged_guide_preview(state, context=architect.guide_context,
        proposal=architect.proposal, publication=architect.guide_result, active=architect.message_lock.locked())
    if preview is not None:
        state["live_design"] = preview
    return state


@app.get("/api/guide/{identity}/timeline")
async def guide_conversation(identity: str, after: str | None = None, limit: int = 500):
    if not 1 <= limit <= 500:
        raise HTTPException(422, "Use a conversation page size between 1 and 500.")
    return await guide_result(guide_timeline.read(identity, after=after, limit=limit))


@app.post("/api/guide/{identity}/attachments")
async def upload_guide_attachments(identity: str, request: Request):
    await guide_result(asyncio.to_thread(guide.attachments._known, identity))
    # Bound the complete multipart body while Starlette parses it, including
    # requests without a trustworthy Content-Length header.
    receive, received = request._receive, 0
    async def bounded_receive():
        nonlocal received
        message = await receive()
        received += len(message.get("body", b""))
        if received > MAX_TURN_BYTES + 64 * 1024:
            raise MultiPartException("Attachments together must be at most 16 MiB.")
        return message
    request._receive = bounded_receive
    try:
        async with request.form(max_files=MAX_FILES, max_fields=0, max_part_size=MAX_FILE_BYTES) as form:
            uploads = form.getlist("files")
            if len(form.multi_items()) != len(uploads) or not 1 <= len(uploads) <= MAX_FILES:
                raise HTTPException(422, "Choose between one and four files using the files upload field.")
            files = []
            for upload in uploads:
                if not hasattr(upload, "read"):
                    raise HTTPException(422, "Upload actual files using the files field.")
                data = await upload.read(MAX_FILE_BYTES + 1)
                files.append((upload.filename, upload.content_type, data))
            references = await guide_result(asyncio.to_thread(guide.attachments.save, identity, files))
        return {"attachments": references}
    except StarletteHTTPException as error:
        if error.status_code == 400:
            raise HTTPException(422, "The upload exceeds its limits or is not valid multipart file data.") from None
        raise
    finally:
        request._receive = receive


@app.get("/api/guide/{identity}/attachments/{attachment_id}")
async def download_guide_attachment(identity: str, attachment_id: str):
    reference, data = await guide_result(asyncio.to_thread(guide.attachments.read, identity, attachment_id))
    return Response(data, media_type=reference["media_type"], headers={
        "Content-Disposition": "attachment; filename*=UTF-8''" + quote(reference["filename"], safe=""),
        "Cache-Control": "private, no-store", "X-Content-Type-Options": "nosniff"})


@app.post("/api/guide/{identity}/respond")
async def respond_guide(identity: str, value: GuideResponse):
    if value.blueprint is not None:
        value.blueprint = blueprint_or_error(value.blueprint).model_dump(mode="json")
    if value.manual_intent is not None:
        base = blueprint_or_error(value.manual_intent.base).model_dump(mode="json")
        draft = blueprint_or_error(value.manual_intent.draft).model_dump(mode="json")
        if base == draft:
            raise HTTPException(422, "Adjust at least one detail before asking the guide to review your edits.")
    await guide_result(asyncio.to_thread(guide.attachments.resolve, identity, value.attachments))
    state, pending = await guide_result(guide_operations.submit(guide, identity, value))
    return JSONResponse({**state, "input_surface": guide.input_surface()}, status_code=202 if pending else 200)


@app.post("/api/guide/{identity}/apply")
async def apply_guide(identity: str, value: GuideApply):
    return await guide_result(guide.apply(identity, value.version))


class SavedToolkitPreview(BaseModel):
    model_config = ConfigDict(extra='forbid')
    version: int = Field(ge=1, strict=True)
    manifest_sha256: str = Field(pattern=r'^[0-9a-f]{64}$', strict=True)
    request_id: str = Field(pattern=r'^[0-9a-f]{32}$', strict=True)


class SavedToolkitReview(SavedToolkitPreview):
    request_sha256: str = Field(pattern=r'^[0-9a-f]{64}$', strict=True)
    host_fingerprint: str = Field(pattern=r'^[0-9a-f]{64}$', strict=True)
    audience_approved: bool = Field(strict=True)


async def saved_toolkit_guide(identity, version=None):
    state = await guide_result(guide_operations.read(guide, identity))
    if version is not None:
        if version != state['version']:
            raise HTTPException(409, 'This setup changed. Reopen Saved tools before reviewing a request.')
        if state.get('operation', {}).get('status') == 'running':
            raise HTTPException(409, 'Wait for the current guide response before reviewing saved tools.')
    return state


async def saved_toolkit_result(operation):
    try:
        return await operation
    except ToolkitSavedRequestsError as error:
        raise HTTPException(422, str(error)) from None


@app.get('/api/guide/{identity}/toolkit-saved')
async def read_saved_toolkit(identity: str):
    await saved_toolkit_guide(identity)
    return await asyncio.to_thread(toolkit_saved_requests.status, identity)


@app.post('/api/guide/{identity}/toolkit-saved/manifest')
async def inspect_saved_toolkit_manifest(identity: str, request: Request, version: int = Query(ge=1)):
    await saved_toolkit_guide(identity, version)
    raw = await request.body()
    if not 1 <= len(raw) <= MAX_MANIFEST_BYTES:
        raise HTTPException(422, 'Choose a saved Toolkit recovery JSON file of at most 64 KiB.')
    return await saved_toolkit_result(toolkit_saved_requests.inspect(identity, raw))


@app.post('/api/guide/{identity}/toolkit-saved/preview')
async def preview_saved_toolkit(identity: str, value: SavedToolkitPreview):
    await saved_toolkit_guide(identity, value.version)
    return await saved_toolkit_result(toolkit_saved_requests.preview(
        identity, value.manifest_sha256, value.request_id))


@app.post('/api/guide/{identity}/toolkit-saved/review', status_code=202)
async def review_saved_toolkit(identity: str, value: SavedToolkitReview):
    await saved_toolkit_guide(identity, value.version)
    # Use the existing guide owners while admission verifies the host binding.
    # Another tab cannot publish a new turn between this check and preparation.
    async with guide_operations.lock:
        async with guide.lock:
            current = await asyncio.to_thread(guide.store.get, identity)
            operation = await asyncio.to_thread(guide.store.operation, identity)
            if current is None:
                raise HTTPException(404, 'This saved setup could not be found.')
            if current['version'] != value.version or operation and operation.get('status') == 'running':
                raise HTTPException(409, 'This setup changed. Reopen Saved tools before reviewing a request.')
            return await saved_toolkit_result(toolkit_saved_requests.review(
                identity, value.version, value.manifest_sha256, value.request_id, value.request_sha256,
                host_fingerprint=value.host_fingerprint, audience_approved=value.audience_approved))


class ImplementationRequest(BaseModel):
    model_config = ConfigDict(extra="forbid")
    version: int = Field(ge=1, strict=True)
    request: str = Field(default="", max_length=6000)
    previous_build_id: str | None = Field(default=None, min_length=1, max_length=64, strict=True)
    refresh_framework: bool = Field(default=False, strict=True)
    preserve_design: bool = Field(default=False, strict=True)
    start_fresh: bool = Field(default=False, strict=True)
    previous_source_digest: str | None = Field(default=None, pattern="^[a-f0-9]{64}$", strict=True)

    @model_validator(mode="after")
    def exact_runtime_refresh(self):
        if self.start_fresh:
            if (self.refresh_framework or self.preserve_design or self.request.strip()
                    or self.previous_build_id is None or self.previous_source_digest is None):
                raise ValueError("A fresh build needs its exact incomplete predecessor and no repair or refresh request.")
        elif self.previous_source_digest is not None:
            raise ValueError("A source digest is only selected for an explicit fresh build.")
        if self.preserve_design and (not self.refresh_framework or self.request.strip()
                or self.previous_build_id is None):
            raise ValueError("Runtime refresh requires a selected accepted build, framework refresh, and no feedback.")
        return self


class ImplementationSelectionRequest(BaseModel):
    model_config = ConfigDict(extra="forbid")
    version: int = Field(ge=1, strict=True)
    build_id: str = Field(min_length=36, max_length=36, strict=True)
    selection_token: str = Field(pattern="^[a-f0-9]{64}$", strict=True)


class ImplementationStopRequest(BaseModel):
    model_config = ConfigDict(extra="forbid")
    version: int = Field(ge=1, strict=True)
    build_id: str = Field(min_length=1, max_length=64, strict=True)


class DeploymentRequest(BaseModel):
    model_config = ConfigDict(extra="forbid")
    version: int = Field(ge=1, strict=True)
    build_id: str | None = Field(default=None, min_length=1, max_length=64)
    source_kind: str = Field(default="application", pattern="^(application|native-team)$")
    source_id: str | None = Field(default=None, pattern="^[a-f0-9]{64}$")
    source_digest: str | None = Field(default=None, pattern="^[a-f0-9]{64}$")
    destination: str = Field(min_length=1, max_length=4096)
    settings: dict[str, str] = Field(default_factory=dict, max_length=200)
    use_host_provider_credentials: bool = Field(default=False, strict=True)
    execution_mode: str = Field(default="interactive", pattern="^(interactive|background)$")

    @model_validator(mode="after")
    def selected_source(self):
        if self.source_kind == "native-team":
            if self.build_id is not None or self.source_id is None or self.source_digest is None:
                raise ValueError("Choose the exact compiled agent-team source, without an application build ID.")
        elif self.build_id is None or self.source_id is not None or self.source_digest is not None:
            raise ValueError("Choose the independently accepted application build ID.")
        return self


class RemoteConnectRequest(BaseModel):
    model_config = ConfigDict(extra="forbid")
    version: int = Field(ge=1, strict=True)
    build_id: str = Field(min_length=1, max_length=64, strict=True)
    host: str = Field(min_length=1, max_length=253, strict=True)
    user: str = Field(min_length=1, max_length=64, strict=True)
    ssh_port: int = Field(default=22, ge=1, le=65535, strict=True)
    destination: str = Field(min_length=1, max_length=2000, strict=True)


class GcpAccountRequest(BaseModel):
    model_config = ConfigDict(extra="forbid")
    version: int = Field(ge=1, strict=True)
    build_id: str = Field(min_length=1, max_length=64, strict=True)
    account: str = Field(min_length=1, max_length=320, pattern=r"^[A-Za-z0-9][^\s\x00-\x1f]*$", strict=True)


class GcpCheckRequest(GcpAccountRequest):
    project: str = Field(pattern=r"^[a-z][a-z0-9-]{4,28}[a-z0-9]$", strict=True)


class GcpCreateRequest(GcpCheckRequest):
    request_id: str = Field(pattern="^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$", strict=True)


class GcpResumeRequest(BaseModel):
    model_config = ConfigDict(extra="forbid")
    operation_id: str = Field(pattern="^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$", strict=True)


class GcpConnectRequest(GcpResumeRequest):
    version: int = Field(ge=1, strict=True)
    build_id: str = Field(min_length=1, max_length=64, strict=True)


class GcpRemoveRequest(GcpResumeRequest):
    request_id: str = Field(pattern="^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$", strict=True)
    acknowledge_data_loss: bool = Field(strict=True)


class RemoteInstallRequest(BaseModel):
    model_config = ConfigDict(extra="forbid")
    version: int = Field(ge=1, strict=True)
    build_id: str = Field(min_length=1, max_length=64, strict=True)
    deployment_id: str = Field(min_length=1, max_length=64, strict=True)
    settings: dict[str, str] = Field(default_factory=dict, max_length=100)


class RemoteRetryRequest(RemoteInstallRequest):
    previous_operation_id: str = Field(pattern="^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$", strict=True)


class RemoteActionRequest(BaseModel):
    model_config = ConfigDict(extra="forbid")
    deployment_id: str = Field(min_length=1, max_length=64, strict=True)
    action: str = Field(pattern="^(prepare-setup|finish-setup|start-background|stop|restart)$", strict=True)
    action_request_id: str | None = Field(default=None, pattern="^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$", strict=True)


class RemoteSettingsRequest(BaseModel):
    model_config = ConfigDict(extra="forbid")
    version: int = Field(ge=1, strict=True)
    deployment_id: str = Field(pattern="^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$", strict=True)
    build_id: str = Field(min_length=1, max_length=64, strict=True)
    source_digest: str = Field(pattern="^[a-f0-9]{64}$", strict=True)
    package_sha256: str = Field(pattern="^[a-f0-9]{64}$", strict=True)
    settings: dict[str, str] = Field(default_factory=dict, max_length=100)
    remove_fields: list[str] = Field(default_factory=list, max_length=100)
    action_request_id: str | None = Field(default=None, pattern="^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$", strict=True)


class RemoteBrowserOpenRequest(BaseModel):
    model_config = ConfigDict(extra="forbid")
    version: int = Field(ge=1, strict=True)
    build_id: str = Field(min_length=1, max_length=64, strict=True)
    deployment_id: str = Field(pattern="^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$", strict=True)
    entry_id: str = Field(pattern="^(native-console|application:[a-z][a-z0-9_-]{0,63})$", strict=True)


class RemoteBrowserCloseRequest(BaseModel):
    model_config = ConfigDict(extra="forbid")
    deployment_id: str = Field(pattern="^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$", strict=True)
    access_id: str = Field(pattern="^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$", strict=True)


class DeploymentSettingsRequest(BaseModel):
    model_config = ConfigDict(extra="forbid")
    version: int = Field(ge=1, strict=True)
    deployment_id: str = Field(min_length=1, max_length=64)
    build_id: str | None = Field(default=None, min_length=1, max_length=64)
    source_kind: str | None = Field(default=None, pattern="^(application|native-team)$")
    source_id: str | None = Field(default=None, pattern="^[a-f0-9]{64}$")
    source_digest: str = Field(pattern="^[a-f0-9]{64}$")
    package_sha256: str = Field(pattern="^[a-f0-9]{64}$")
    settings: dict[str, str] = Field(default_factory=dict, max_length=100)
    remove_fields: list[str] = Field(default_factory=list, max_length=100)
    use_host_provider_credentials: bool = Field(default=False, strict=True)

    @model_validator(mode="after")
    def removal_names(self):
        import re
        if self.source_kind == "native-team":
            if self.build_id is not None or self.source_id is None:
                raise ValueError("Choose the exact installed agent-team source.")
        elif self.build_id is None or self.source_id is not None:
            raise ValueError("Choose the exact installed application build ID.")
        if (any(not re.fullmatch(r"[A-Z][A-Z0-9_]{0,119}", key) for key in self.remove_fields)
                or len(set(self.remove_fields)) != len(self.remove_fields)):
            raise ValueError("Choose each declared optional setting to remove once.")
        return self


class DeploymentAction(BaseModel):
    model_config = ConfigDict(extra="forbid")
    deployment_id: str = Field(min_length=1, max_length=64)
    action: str = Field(pattern="^(stop|restart|retry|apply-update|rollback|recover-current|prepare-support|apply-support|prepare-setup|finish-setup|start-background)$", strict=True)
    update_id: str | None = Field(default=None, min_length=1, max_length=64)
    action_request_id: str | None = Field(default=None, strict=True,
        pattern=r"^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$")
    expected_service_digest: str | None = Field(default=None, strict=True, pattern=r"^[a-f0-9]{64}$")
    revision: str | None = Field(default=None, strict=True,
        pattern=r"^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$")
    descriptor_sha256: str | None = Field(default=None, strict=True, pattern=r"^[a-f0-9]{64}$")
    resume: bool = Field(default=False, strict=True)

    @model_validator(mode="after")
    def require_selected_update(self):
        update_actions = {"apply-update", "rollback", "recover-current"}
        if self.action in update_actions and self.update_id is None:
            raise ValueError("Choose the exact update before applying, restoring, or recovering it.")
        if self.action not in update_actions and self.update_id is not None:
            raise ValueError("An update identity is only used to apply, restore, or recover an update.")
        support_actions = {"prepare-support", "apply-support"}
        if self.action in {"prepare-setup", "finish-setup", "start-background"} and self.action_request_id is None:
            raise ValueError("Starting in background requires an exact action request UUID." if self.action == "start-background"
                else "Finishing setup requires an exact action request UUID." if self.action == "finish-setup"
                else "Preparing setup requires an exact action request UUID.")
        if self.action in support_actions and self.action_request_id is None:
            raise ValueError("Installation support requires an exact action request UUID.")
        if self.action == "prepare-support":
            if self.expected_service_digest is None or self.revision is not None or self.descriptor_sha256 is not None:
                raise ValueError("Preparing support requires only the exact current service digest.")
        elif self.action == "apply-support":
            if self.expected_service_digest is not None or self.revision is None or self.descriptor_sha256 is None:
                raise ValueError("Applying support requires only the exact prepared revision and descriptor digest.")
        elif self.resume or any(value is not None for value in (self.expected_service_digest, self.revision, self.descriptor_sha256)):
            raise ValueError("Support fields are only used for installation support actions.")
        return self


class DeploymentUpdateRequest(BaseModel):
    model_config = ConfigDict(extra="forbid")
    deployment_id: str = Field(min_length=1, max_length=64)
    version: int = Field(ge=1, strict=True)
    build_id: str = Field(min_length=1, max_length=64)


class InstalledFeedbackRequest(BaseModel):
    model_config = ConfigDict(extra="forbid")
    version: int = Field(ge=1, strict=True)
    deployment_id: str = Field(min_length=1, max_length=64, strict=True)
    build_id: str = Field(min_length=1, max_length=64, strict=True)
    source_digest: str = Field(pattern="^[0-9a-f]{64}$", strict=True)
    package_sha256: str = Field(pattern="^[0-9a-f]{64}$", strict=True)
    continuation_build_id: str = Field(min_length=1, max_length=64, strict=True)
    continuation_source_digest: str = Field(pattern="^[0-9a-f]{64}$", strict=True)
    continue_current_candidate: bool = Field(default=False, strict=True)
    outcome: str = Field(pattern="^(worked|partly|not_worked|blocked)$", strict=True)
    request: str = Field(min_length=1, max_length=6000, strict=True)
    task: str = Field(default="", max_length=3000, strict=True)


class RemoteInstalledFeedbackRequest(BaseModel):
    model_config = ConfigDict(extra="forbid")
    version: int = Field(ge=1, strict=True)
    remote_binding: dict = Field(min_length=1, max_length=24)
    continuation_build_id: str = Field(min_length=1, max_length=64, strict=True)
    continuation_source_digest: str = Field(pattern="^[0-9a-f]{64}$", strict=True)
    continue_current_candidate: bool = Field(default=False, strict=True)
    outcome: str = Field(pattern="^(worked|partly|not_worked|blocked)$", strict=True)
    request: str = Field(min_length=1, max_length=6000, strict=True)
    task: str = Field(default="", max_length=3000, strict=True)


class InstalledSuccessRequest(BaseModel):
    model_config = ConfigDict(extra="forbid")
    version: int = Field(ge=1, le=2 ** 53 - 1, strict=True)
    deployment_id: str = Field(min_length=1, max_length=64, strict=True)
    build_id: str = Field(min_length=1, max_length=64, strict=True)
    source_digest: str = Field(pattern="^[0-9a-f]{64}$", strict=True)
    package_sha256: str = Field(pattern="^[0-9a-f]{64}$", strict=True)
    request_id: str = Field(pattern="^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$", strict=True)
    task: str = Field(min_length=1, max_length=3000, strict=True)

    @model_validator(mode="after")
    def meaningful_task(self):
        if (not self.task.strip() or any(ord(char) == 127 or ord(char) < 32 and char not in "\n\r\t"
                                       or 0xD800 <= ord(char) <= 0xDFFF for char in self.task)):
            raise ValueError("Describe a meaningful task with ordinary text.")
        return self


class RehearsalRequest(BaseModel):
    model_config = ConfigDict(extra="forbid")
    version: int = Field(ge=1, strict=True)
    text: str = Field(default="", max_length=6000)
    flow_id: str | None = Field(default=None, max_length=80)
    params: dict | None = None
    fixtures: dict | None = None
    mode: str = Field(default="design", pattern="^(design|application)$")
    build_id: str | None = Field(default=None, min_length=1, max_length=64)
    application_fixtures: dict | None = None
    keep_open: bool = Field(default=False, strict=True)
    start_request_id: str | None = Field(default=None, strict=True,
        pattern=r"^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$")


class RehearsalContinue(BaseModel):
    model_config = ConfigDict(extra="forbid")
    version: int = Field(ge=1, strict=True)
    rehearsal_id: str = Field(min_length=1, max_length=64, strict=True)
    expected_turn: int = Field(ge=1, le=9, strict=True)
    text: str = Field(min_length=1, max_length=6000, strict=True)


class RehearsalCancel(BaseModel):
    model_config = ConfigDict(extra="forbid")
    rehearsal_id: str = Field(min_length=1, max_length=64)


class RehearsalRepair(BaseModel):
    model_config = ConfigDict(extra="forbid")
    version: int = Field(ge=1, strict=True)
    rehearsal_id: str = Field(min_length=1, max_length=64)
    build_id: str = Field(min_length=1, max_length=64)
    source_digest: str = Field(pattern="^[0-9a-f]{64}$")
    request: str = Field(default="", max_length=6000, strict=True)


TRIAL_REPLAY_UUID = r"^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$"


class RehearsalReplayRequest(BaseModel):
    """Public replay bindings only; the host retrieves the original job inputs."""
    model_config = ConfigDict(extra="forbid")
    version: int = Field(ge=1, strict=True)
    original_build_id: str = Field(pattern=TRIAL_REPLAY_UUID, strict=True)
    original_source_digest: str = Field(pattern="^[0-9a-f]{64}$", strict=True)
    build_id: str = Field(pattern=TRIAL_REPLAY_UUID, strict=True)
    source_digest: str = Field(pattern="^[0-9a-f]{64}$", strict=True)
    start_request_id: str = Field(pattern=TRIAL_REPLAY_UUID, strict=True)


@app.post("/api/guide/{identity}/rehearsal/{rehearsal_id}/replay", status_code=202)
async def replay_application_trial(identity: str, value: RehearsalReplayRequest,
        rehearsal_id: str = ApiPath(pattern=TRIAL_REPLAY_UUID)):
    from .rehearsal import ReplayNotAdmitted, replay_rejection_matches
    async def admitted_or_rejected():
        try:
            return await rehearsal.replay(identity, value.version, rehearsal_id,
                value.original_build_id, value.original_source_digest, value.build_id, value.source_digest,
                value.start_request_id)
        except (LookupError, DraftConflict, ArchitectUnavailable, ValueError) as error:
            witness = getattr(error, "trial_start_rejection", None)
            if (isinstance(witness, ReplayNotAdmitted) and witness.http_status in {404, 409, 422, 503}
                    and replay_rejection_matches(witness, identity, value.version, rehearsal_id,
                        value.original_build_id, value.original_source_digest, value.build_id,
                        value.source_digest, value.start_request_id)):
                return JSONResponse(status_code=witness.http_status,
                    content={"detail": str(error), "start_rejection": witness.projection()})
            raise
    return await guide_result(admitted_or_rejected())


@app.post("/api/guide/{identity}/rehearsal", status_code=202)
async def start_rehearsal(identity: str, value: RehearsalRequest):
    async def admitted_or_rejected():
        try:
            return await rehearsal.submit(identity, value.version, value.text,
                flow_id=value.flow_id, params=value.params, fixtures=value.fixtures, mode=value.mode,
                build_id=value.build_id, application_fixtures=value.application_fixtures, keep_open=value.keep_open,
                start_request_id=value.start_request_id)
        except (LookupError, DraftConflict, ArchitectUnavailable, ValueError) as error:
            witness = getattr(error, "trial_start_rejection", None)
            if (isinstance(witness, StartNotAdmitted) and value.start_request_id is not None
                    and witness.witness_version == 1 and witness.status == "rejected"
                    and witness.start_request_complete is False and witness.http_status in {404, 409, 422, 503}
                    and witness.guide_id == identity and witness.guide_version == value.version
                    and witness.mode == value.mode and witness.start_request_id == value.start_request_id
                    and witness.start_request_fingerprint == start_request_fingerprint(identity, value.version, value.text,
                        flow_id=value.flow_id, params=value.params, fixtures=value.fixtures, mode=value.mode,
                        build_id=value.build_id, application_fixtures=value.application_fixtures, keep_open=value.keep_open)):
                return JSONResponse(status_code=witness.http_status,
                    content={"detail": str(error), "start_rejection": witness.projection()})
            raise
    return await guide_result(admitted_or_rejected())


@app.post("/api/guide/{identity}/rehearsal/continue", status_code=202)
async def continue_rehearsal(identity: str, value: RehearsalContinue):
    return await guide_result(rehearsal.continue_trial(identity, value.version, value.rehearsal_id,
        value.expected_turn, value.text))


@app.post("/api/guide/{identity}/rehearsal/end", status_code=202)
async def end_rehearsal(identity: str, value: RehearsalCancel):
    return await guide_result(rehearsal.end(identity, value.rehearsal_id))


@app.get("/api/guide/{identity}/rehearsal")
async def rehearsal_status(identity: str):
    receipt = await guide_result(rehearsal.status(identity))
    return {**receipt, "browser_console_url": await browser_console_url(rehearsal, identity, receipt, "rehearsal")}


@app.get("/api/guide/{identity}/rehearsal/timeline")
async def rehearsal_timeline(identity: str, after: str | None = None, limit: int = 200,
                             rehearsal_id: str | None = None):
    return await guide_result(rehearsal.timeline(identity, after, limit, rehearsal_id))


@app.post("/api/guide/{identity}/rehearsal/cancel")
async def cancel_rehearsal(identity: str, value: RehearsalCancel):
    return await guide_result(rehearsal.cancel(identity, value.rehearsal_id))


@app.post("/api/guide/{identity}/rehearsal/repair", status_code=202)
async def repair_application_trial(identity: str, value: RehearsalRepair):
    from .trial_feedback import repair_trial
    return await guide_result(repair_trial(rehearsal, implementation, identity, value.version,
        value.rehearsal_id, value.build_id, value.source_digest, request=value.request))


@app.get("/api/guide/{identity}/deployment/options")
async def deployment_options(identity: str, build_id: str | None = None,
                             console_port: int | None = Query(default=None, ge=1024, le=65535),
                             source_kind: str = Query(default="application", pattern="^(application|native-team)$"),
                             version: int | None = Query(default=None, ge=1)):
    extra = {"console_port": console_port} if console_port is not None else {}
    if source_kind == "native-team" or version is not None:
        extra.update(source_kind=source_kind, version=version)
    return await guide_result(deployment.options(identity, build_id, **extra))


@app.get("/api/guide/{identity}/deployment/package")
async def native_deployment_package(identity: str, version: int = Query(ge=1),
                                    source_kind: str = Query(pattern="^native-team$")):
    payload, _, slug = await guide_result(deployment.native_package(identity, version))
    return Response(payload, media_type="application/zip",
                    headers={"Content-Disposition": f'attachment; filename="{slug}-native-team.zip"'})


@app.get("/api/guide/{identity}/gcp-preflight/options")
async def gcp_preflight_options(identity: str, build_id: str = Query(min_length=1, max_length=64)):
    return await guide_result(gcp_preflight.options(identity, build_id))


@app.post("/api/guide/{identity}/gcp-preflight/projects")
async def gcp_preflight_projects(identity: str, value: GcpAccountRequest):
    return await guide_result(gcp_preflight.projects(identity, value.version, value.build_id, value.account))


@app.post("/api/guide/{identity}/gcp-preflight/check")
async def check_gcp_preflight(identity: str, value: GcpCheckRequest):
    return await guide_result(gcp_preflight.check(identity, value.version, value.build_id, value.account, value.project))


@app.post("/api/guide/{identity}/gcp-preflight/check-guided")
async def check_gcp_guided(identity: str, value: GcpCheckRequest):
    return await guide_result(gcp_preflight.check_guided(identity, value.version, value.build_id, value.account, value.project))


@app.get("/api/guide/{identity}/gcp-deployment")
async def gcp_deployment_status(identity: str, operation_id: str | None = None):
    return await guide_result(gcp_provisioning.status(identity, operation_id))


@app.post("/api/guide/{identity}/gcp-deployment/create", status_code=202)
async def create_gcp_deployment(identity: str, value: GcpCreateRequest):
    return await guide_result(gcp_provisioning.create(identity, value.version, value.build_id, value.account, value.project, value.request_id))


@app.post("/api/guide/{identity}/gcp-deployment/resume", status_code=202)
async def resume_gcp_deployment(identity: str, value: GcpResumeRequest):
    return await guide_result(gcp_provisioning.resume(identity, value.operation_id))


@app.post("/api/guide/{identity}/gcp-deployment/remove", status_code=202)
async def remove_gcp_deployment(identity: str, value: GcpRemoveRequest):
    if value.acknowledge_data_loss is not True:
        raise HTTPException(422, "Confirm that the original boot disk's data may be permanently deleted before removing these resources.")
    return await guide_result(gcp_provisioning.remove(identity, value.operation_id,
        value.request_id, value.acknowledge_data_loss))


@app.post("/api/guide/{identity}/gcp-deployment/connect", status_code=202)
async def connect_gcp_deployment(identity: str, value: GcpConnectRequest):
    async def connect():
        cloud = await gcp_provisioning.status(identity, value.operation_id)
        return await remote_deployment.connect_cloud(identity, value.version, value.build_id, cloud,
            product_owner=_selected_remote_product_owner())
    return await guide_result(connect())


@app.get("/api/guide/{identity}/remote-deployment")
async def remote_deployment_status(identity: str, force: bool = False):
    return await guide_result(remote_deployment.status(identity, force=force))


@app.post("/api/guide/{identity}/remote-deployment/connect", status_code=202)
async def connect_remote_deployment(identity: str, value: RemoteConnectRequest):
    return await guide_result(remote_deployment.connect(identity, value.version, value.build_id,
        value.host, value.user, value.ssh_port, value.destination,
        product_owner=_selected_remote_product_owner()))


@app.post("/api/guide/{identity}/remote-deployment/install", status_code=202)
async def install_remote_deployment(identity: str, value: RemoteInstallRequest):
    return await guide_result(remote_deployment.install(identity, value.version, value.build_id,
        value.deployment_id, value.settings, product_owner=_selected_remote_product_owner()))


@app.post("/api/guide/{identity}/remote-deployment/retry", status_code=202)
async def retry_remote_deployment(identity: str, value: RemoteRetryRequest):
    return await guide_result(remote_deployment.retry_install(identity, value.version, value.build_id,
        value.deployment_id, value.previous_operation_id, value.settings))


@app.post("/api/guide/{identity}/remote-deployment/action", status_code=202)
async def operate_remote_deployment(identity: str, value: RemoteActionRequest):
    owner = _selected_remote_product_owner()
    control = {**({"product_owner": owner} if owner is not None else {}),
        **({"action_request_id": value.action_request_id} if value.action_request_id is not None else {})}
    return await guide_result(remote_deployment.action(identity, value.deployment_id, value.action, **control))


@app.get("/api/guide/{identity}/remote-deployment/settings")
async def remote_installed_deployment_settings(identity: str, deployment_id: str):
    owner = _selected_remote_product_owner()
    return await guide_result(remote_deployment.settings_options(identity, deployment_id,
        **({"product_owner": owner} if owner is not None else {})))


@app.post("/api/guide/{identity}/remote-deployment/settings")
async def configure_remote_installed_deployment(identity: str, value: RemoteSettingsRequest):
    owner = _selected_remote_product_owner()
    control = {**({"product_owner": owner} if owner is not None else {}),
        **({"action_request_id": value.action_request_id} if value.action_request_id is not None else {})}
    return await guide_result(remote_deployment.configure(identity, value.version, value.deployment_id,
        value.build_id, value.source_digest, value.package_sha256,
        settings=value.settings, remove_fields=value.remove_fields, **control))


@app.get("/api/guide/{identity}/remote-deployment/application-setup")
async def remote_application_setup(identity: str, deployment_id: str):
    return await guide_result(remote_deployment.application_setup(identity, deployment_id,
        product_owner=_selected_remote_product_owner()))


@app.get("/api/guide/{identity}/remote-deployment/browser-access")
async def remote_browser_access_status(identity: str, deployment_id: str, entry_id: str = "native-console"):
    return await guide_result(remote_deployment.browser_access.status(identity, deployment_id, entry_id))


@app.post("/api/guide/{identity}/remote-deployment/browser-access/open")
async def open_remote_browser_access(identity: str, value: RemoteBrowserOpenRequest):
    return await guide_result(remote_deployment.browser_access.open(identity, value.version, value.build_id,
        value.deployment_id, value.entry_id))


@app.post("/api/guide/{identity}/remote-deployment/browser-access/close")
async def close_remote_browser_access(identity: str, value: RemoteBrowserCloseRequest):
    return await guide_result(remote_deployment.browser_access.close(identity, value.deployment_id, value.access_id))


@app.get("/api/guide/{identity}/remote-deployment/use")
async def remote_installed_application_use(identity: str, deployment_id: str):
    return await guide_result(remote_deployment.use(identity, deployment_id))


@app.post("/api/guide/{identity}/remote-deployment/use/success")
async def remote_installed_application_success(identity: str, value: InstalledSuccessRequest):
    return await guide_result(remote_deployment.record_success(identity, value.version, value.deployment_id,
        value.build_id, value.source_digest, value.package_sha256, request_id=value.request_id, task=value.task))


@app.post("/api/guide/{identity}/remote-deployment/feedback", status_code=202)
async def remote_installed_application_feedback(identity: str, value: RemoteInstalledFeedbackRequest):
    return await guide_result(remote_deployment.feedback(identity, value.version, value.remote_binding,
        value.continuation_build_id, value.continuation_source_digest, request=value.request,
        outcome=value.outcome, task=value.task, continue_current_candidate=value.continue_current_candidate))


@app.get("/api/guide/{identity}/deployment/installed-package")
async def installed_deployment_package(identity: str, deployment_id: str, build_id: str,
                                       source_digest: str, package_sha256: str):
    archive = await guide_result(deployment.installed_package(identity, deployment_id, build_id,
        source_digest, package_sha256))
    return Response(archive, media_type="application/zip", headers={
        "Content-Disposition": 'attachment; filename="installed-application.zip"'})


@app.get("/api/guide/{identity}/deployment/settings")
async def installed_deployment_settings(identity: str, deployment_id: str,
                                        console_port: int | None = Query(default=None, ge=1024, le=65535)):
    extra = {"console_port": console_port} if console_port is not None else {}
    return await guide_result(deployment.settings_options(identity, deployment_id, **extra))


@app.post("/api/guide/{identity}/deployment/settings")
async def configure_installed_deployment(identity: str, value: DeploymentSettingsRequest):
    async def configure_selected():
        extra = {"source_kind": value.source_kind, "source_id": value.source_id} if value.source_kind is not None else {}
        if application_host is not None:
            extra["application_settings_validator"] = application_host.validate_installed_settings
        return await deployment.configure(identity, value.version, value.deployment_id,
            value.build_id, value.source_digest, value.package_sha256, settings=value.settings,
            remove_fields=value.remove_fields, use_host_provider_credentials=value.use_host_provider_credentials, **extra)
    return await guide_result(configure_selected())


@app.get("/api/guide/{identity}/deployment/application-setup")
async def application_setup_status(identity: str, deployment_id: str):
    async def selected_status():
        if application_host is None:
            settings = await deployment.settings_options(identity, deployment_id)
            return {**application_setup_binding(settings, identity, deployment_id), "available": False,
                "reason": "host_unavailable",
                "detail": "Application setup has no trusted host selection yet."}
        return await application_host.setup_status(deployment, identity, deployment_id)
    return await guide_result(selected_status())


@app.get("/api/guide/{identity}/deployment")
async def deployment_status(identity: str):
    return await guide_result(deployment.status(identity))


@app.get("/api/guide/{identity}/deployment/experiences")
async def deployment_experiences(identity: str):
    return await guide_result(deployment.experiences(identity))


@app.get("/api/guide/{identity}/deployment/use")
async def installed_application_use(identity: str):
    return await guide_result(deployment.use(identity))


@app.post("/api/guide/{identity}/deployment/use/success")
async def installed_application_success(identity: str, value: InstalledSuccessRequest):
    return await guide_result(deployment.record_success(identity, value.version, value.deployment_id,
        value.build_id, value.source_digest, value.package_sha256, request_id=value.request_id, task=value.task))


@app.post("/api/guide/{identity}/deployment/feedback", status_code=202)
async def installed_application_feedback(identity: str, value: InstalledFeedbackRequest):
    return await guide_result(deployment.feedback(identity, value.version, value.deployment_id,
        value.build_id, value.source_digest, value.package_sha256, value.continuation_build_id,
        value.continuation_source_digest, request=value.request, outcome=value.outcome, task=value.task,
        continue_current_candidate=value.continue_current_candidate))


@app.post("/api/guide/{identity}/deployment", status_code=202)
async def install_deployment(identity: str, value: DeploymentRequest):
    async def submit_selected():
        if application_host is not None:
            if type(application_host) is LocalApplicationRegistry:
                if value.source_kind != "native-team":
                    application_host.validate_settings(identity, value.settings, build_id=value.build_id)
            else:
                application_host.validate_settings(identity, value.settings)
        extra = ({"source_kind": value.source_kind, "source_id": value.source_id, "source_digest": value.source_digest}
                 if value.source_kind == "native-team" else {})
        return await deployment.submit(identity, value.version, value.build_id,
            value.destination, value.settings, value.use_host_provider_credentials,
            execution_mode=value.execution_mode, **extra)
    return await guide_result(submit_selected())


@app.post("/api/guide/{identity}/deployment/action")
async def operate_deployment(identity: str, value: DeploymentAction):
    options = {}
    if value.update_id is not None:
        options["update_id"] = value.update_id
    if value.action_request_id is not None:
        options["action_request_id"] = value.action_request_id
    if value.action in {"prepare-support", "apply-support"}:
        options.update(expected_service_digest=value.expected_service_digest,
            revision=value.revision, descriptor_sha256=value.descriptor_sha256)
        if value.resume:
            options["resume"] = True
    return await guide_result(deployment.action(identity, value.deployment_id, value.action, **options),
        failure_detail="The installation command reported an error. Some steps may have finished. Check installation status before choosing another action.",
        timeout_detail="The installation command has not finished reporting. Check installation status before choosing another action.")


@app.get("/api/guide/{identity}/deployment/action/{deployment_id}/{action_request_id}")
async def deployment_action_request(identity: str, deployment_id: str, action_request_id: str):
    return await guide_result(deployment.action_request(identity, deployment_id, action_request_id))


@app.post("/api/guide/{identity}/deployment/update-stage", status_code=202)
async def stage_deployment_update(identity: str, value: DeploymentUpdateRequest):
    return await guide_result(deployment.stage_update(identity, value.deployment_id, value.version, value.build_id))


@app.post("/api/guide/{identity}/implementation", status_code=202)
async def build_implementation(identity: str, value: ImplementationRequest):
    async def submit_selected():
        options = {}
        if (type(application_host) is LocalApplicationRegistry
                and not value.refresh_framework and not value.preserve_design):
            options = application_host.initial_build_options(identity, value.version)
        elif (application_host is not None and value.previous_build_id is None
                and not value.refresh_framework and not value.preserve_design):
            options = application_host.initial_build_options(identity)
        if value.preserve_design:
            checked = await implementation.status(identity)
            if (checked.get("build_id") != value.previous_build_id or checked.get("status") != "completed"
                    or checked.get("current_guide_version") != value.version
                    or any(checked.get(key) is not True for key in ("current", "source_verified",
                        "acceptance_trusted", "acceptance_current", "ready_to_package"))):
                raise DraftConflict("Runtime refresh needs the exact current independently accepted application. Reload its status before continuing.")
            target = await asyncio.to_thread(implementation.selected_runtime_target, identity,
                build_id=value.previous_build_id, source_digest=checked.get("source_digest"))
            from .runtime_selection import MetaToolkitRuntimeTarget
            if type(target) is MetaToolkitRuntimeTarget and application_host is not None:
                if type(application_host) is LocalApplicationRegistry:
                    selected = application_host.initial_build_options(identity, value.version)
                elif type(application_host) is ForgeApplicationHost:
                    selected = application_host.initial_build_options(identity)
                else:
                    raise DraftConflict("Runtime refresh needs the original trusted application source owner.")
                current_target = selected.get("runtime_target")
                if (type(current_target) is not MetaToolkitRuntimeTarget
                        or current_target.target != target.target):
                    raise DraftConflict("Runtime refresh must retain the accepted SDK family and deployment platform.")
                target = current_target
            options = {"preserve_design": True, "runtime_target": target}
        if value.start_fresh:
            options.update(start_fresh=True, expected_fresh_source_digest=value.previous_source_digest)
        return await implementation.submit(identity, value.version, value.request,
            expected_selected_build_id=value.previous_build_id, refresh_framework=value.refresh_framework, **options)
    return await guide_result(submit_selected())


@app.post("/api/guide/{identity}/implementation/stop", status_code=202)
async def stop_implementation(identity: str, value: ImplementationStopRequest):
    return await guide_result(implementation.request_stop(identity, value.version, value.build_id))


@app.get("/api/guide/{identity}/implementation")
async def implementation_status(identity: str):
    receipt = await guide_result(implementation.status(identity))
    return {**receipt, "browser_console_url": await browser_console_url(implementation, identity, receipt, "implementation")}


@app.api_route("/api/owned-console/guide/{generation}/{path:path}", methods=["GET", "HEAD", "POST"])
async def owned_guide_console(generation: str, path: str, request: Request):
    owner = resolve_guide_owner(architect, generation)
    return await console_response(request, owner, path)


@app.api_route("/api/owned-console/{kind}/{identity}/{job_id}/{path:path}", methods=["GET", "HEAD", "POST"])
async def owned_worker_console(kind: str, identity: str, job_id: str, path: str, request: Request):
    service = {"implementation": implementation, "rehearsal": rehearsal}.get(kind)
    if service is None:
        raise HTTPException(404, "Unknown agent Console.")
    owner = await asyncio.to_thread(resolve_owner, service, identity, job_id, kind)
    return await console_response(request, owner, path)


@app.get("/api/guide/{identity}/implementation/saved-selection")
async def saved_implementation_selection(identity: str):
    return await guide_result(implementation.saved_selection(identity))


@app.post("/api/guide/{identity}/implementation/saved-selection")
async def restore_implementation_selection(identity: str, value: ImplementationSelectionRequest):
    return await guide_result(implementation.restore_saved_selection(
        identity, value.version, value.build_id, value.selection_token))


@app.get("/api/guide/{identity}/implementation/timeline")
async def implementation_timeline(identity: str, after: str | None = None,
                                  limit: int = 200, build_id: str | None = None):
    if not 1 <= limit <= 500:
        raise HTTPException(422, "Use a conversation page size between 1 and 500.")
    return await guide_result(implementation.timeline(identity, after, limit, build_id))


@app.get("/api/guide/{identity}/implementation/files")
async def implementation_file(identity: str, path: str, build_id: str | None = None):
    return await guide_result(implementation.file(identity, path, build_id))


@app.get("/api/guide/{identity}/implementation/package")
async def implementation_package(identity: str, build_id: str | None = None):
    archive, slug = await guide_result(implementation.package(identity, build_id))
    return Response(archive, media_type="application/zip", headers={
        "Content-Disposition": f'attachment; filename="{slug}-application.zip"'})


def blueprint_or_error(value):
    from .blueprint import Blueprint
    from pydantic import ValidationError
    try:
        return Blueprint.model_validate(value)
    except ValidationError as error:
        raise HTTPException(422, [{"loc": e["loc"], "msg": e["msg"], "type": e["type"]} for e in error.errors()]) from None


@app.get("/api/workbench")
async def workbench():
    from .blueprint import starter_blueprint
    return {"blueprint": starter_blueprint().model_dump(mode="json"), "capabilities": await capabilities(),
            "architect": architect.status(), "presets": [], "projects": await asyncio.to_thread(projects.list)}


@app.get("/api/blueprint/schema")
async def blueprint_schema():
    from .blueprint import Blueprint
    return Blueprint.model_json_schema()


@app.post("/api/blueprint/validate")
async def validate_blueprint(value: dict):
    from .compiler import package_preview
    spec = blueprint_or_error(value)
    try:
        return await asyncio.to_thread(package_preview, spec)
    except ValueError as error:
        raise HTTPException(422, str(error)) from None


def require_agent_team_export():
    """Refuse an agent team package where this Forge does not offer one."""
    availability = deployment.agent_team_availability()
    if not availability["available"]:
        raise HTTPException(409, availability["reason"])


@app.post("/api/blueprint/package")
async def blueprint_package(value: dict):
    from .compiler import build_archive
    require_agent_team_export()
    spec = blueprint_or_error(value)
    try:
        archive = await asyncio.to_thread(build_archive, spec)
    except ValueError as error:
        raise HTTPException(422, str(error)) from None
    return Response(archive, media_type="application/zip", headers={"Content-Disposition": f'attachment; filename="{spec.project.slug}.zip"'})


@app.post("/api/blueprint/import")
async def import_blueprint(value: dict):
    from .blueprint import from_v1
    warnings = []
    if value.get("schema_version") == 1:
        try:
            spec = from_v1(value)
        except ValueError as error:
            raise HTTPException(422, str(error)) from None
        if any(c in value.get("connections", []) for c in ("github", "telegram")):
            warnings.append("Legacy GitHub and Telegram connections need MCP selection in the new workbench. Their original package remains unchanged.")
    else:
        spec = blueprint_or_error(value)
    return {"blueprint": spec.model_dump(mode="json"), "warnings": warnings}


@app.get("/api/connectors/search")
async def connector_search(q: str = "", limit: int = 12):
    from .connector_discovery import search_registry
    if not q.strip() or len(q) > 100 or not 1 <= limit <= 30:
        raise HTTPException(422, "Use a short connector name and a result limit between 1 and 30.")
    try:
        return await search_registry(q, limit=limit)
    except Exception as error:
        LOG.warning("MCP registry search failed (%s)", type(error).__name__)
        raise HTTPException(502, "The MCP Registry could not be reached. Retry or import a connector's documented MCP configuration.") from None


@app.post("/api/connectors/import")
async def connector_import(value: dict):
    from .connector_discovery import normalize_custom_config
    try:
        return normalize_custom_config(value)
    except ValueError as error:
        raise HTTPException(422, str(error)) from None


@app.post("/api/connectors/inspect")
async def inspect_connection(value: InspectConnection):
    from .blueprint import Connection, PLACEHOLDER
    from .templates_v2.mcp_connections import MCPConnection
    from pydantic import ValidationError
    try:
        config = Connection.model_validate(value.connection).model_dump(mode="json")
    except ValidationError as error:
        raise HTTPException(422, [{"loc": e["loc"], "msg": e["msg"], "type": e["type"]} for e in error.errors()]) from None
    import json
    missing = sorted({key for key in PLACEHOLDER.findall(json.dumps(config)) if not os.getenv(key)})
    if missing:
        raise HTTPException(422, "Configure these values in the studio server environment to inspect here, or run --connections-check on the destination: " + ", ".join(missing))
    connection = MCPConnection(config)
    try:
        await connection.start()
        return {"connected": True, "connection_id": config["id"], "tool_count": len(connection.tools),
                "tools": [{"name": tool.name, "description": tool.description or "", "input_schema": tool.inputSchema,
                           "read_only": getattr(tool.annotations, "readOnlyHint", None),
                           "destructive": getattr(tool.annotations, "destructiveHint", None)} for tool in connection.tools.values()]}
    except Exception as error:
        LOG.warning("MCP inspection failed (%s)", type(error).__name__)
        raise HTTPException(502, "The MCP handshake failed. Check the configured executable, endpoint, and local authentication.") from None
    finally:
        await connection.close()


@app.get("/api/connectors/catalog")
async def connector_catalog():
    from .catalog import connector_catalog
    return connector_catalog()


@app.get("/api/knowledge/capabilities")
async def knowledge_capability():
    from .elephant_setup import elephant_capability
    return elephant_capability()


@app.get("/api/architect/status")
async def architect_status():
    try:
        return await architect.refresh_status()
    except Exception:
        return architect.status()


def provider_owned_work():
    """Keep credentials fixed for every currently owned native operation."""
    if (architect.message_lock.locked() or architect.pending_interaction
            or architect.visualizer.lock.locked() or architect.visualizer.pending
            or visual_review_runtime.runtime is not None or visual_review_runtime.lock.locked()):
        return True
    owners = (guide_operations, visualization_operations, visualization_reviews,
              implementation, rehearsal, deployment, remote_deployment, gcp_provisioning,
              toolkit_saved_requests)
    if toolkit_saved_requests.setups:
        return True
    return any(getattr(owner, 'tasks', {}) or getattr(owner, 'processes', {}) for owner in owners)


def provider_public_status():
    from .architect import setup_runtime_root, setup_settings_root
    public = provider_settings.status(runtime_active=architect.runtime is not None or architect.state == 'starting')
    from .bootstrap import selected_guide_runtime
    selected = selected_guide_runtime()
    route = selected.model_configuration if selected is not None else None
    if route is not None:
        # This reports host configuration, not a completed provider invocation.
        # Credential contents and native config locators stay in host custody.
        for row in public['providers']:
            if row['provider'] == route['provider']:
                row.update(configured=True, source='native_host_config', editable=False)
        public['effective_provider'] = route['provider']
        public['effective_model'] = route['model']
    settings_root = setup_settings_root(STUDIO_ROOT)
    lent = settings_root != setup_runtime_root(STUDIO_ROOT)
    try:
        locked = canonical_setup_provider(settings_root)
        binding_error = None
    except ProviderSettingsUnavailable as error:
        # A previous generation only lends its settings; it never binds a fresh one.
        locked, binding_error = None, (None if lent else str(error))
    if lent and route is not None and locked is not None and locked != route['provider']:
        # The selected model host binds a fresh generation; a lent provider never conflicts with it.
        locked = None
    if locked:
        public['effective_provider'] = locked if any(
            row['provider'] == locked and row['configured'] for row in public['providers']) else None
    if route is not None:
        if locked is not None and locked != route['provider']:
            binding_error = 'The saved setup provider differs from this selected model host. Preserve its native state.'
        locked = route['provider']
    return {**public, 'locked_provider': locked, 'provider_binding_error': binding_error,
            'work_active': bool(provider_owned_work() or architect.provider_stop_failed),
            'runtime_status': architect.state}


class ProviderSettingsBody(BaseModel):
    model_config = ConfigDict(extra='forbid')
    provider: str = Field(pattern='^(openai|anthropic|gemini)$', strict=True)
    api_key: str = Field(min_length=1, max_length=4096, strict=True, repr=False)


@app.get('/api/provider-settings')
async def read_provider_settings():
    return provider_public_status()


@app.get('/api/checker-setup')
async def read_checker_setup():
    return await checker_setup.status()


@app.post('/api/checker-setup', status_code=202)
async def prepare_checker_setup(request: Request):
    if await request.body():
        raise HTTPException(422, 'Checker preparation does not accept a command, path or configuration.')
    if implementation.lock.locked() or implementation.tasks or implementation.processes:
        raise HTTPException(409, 'Wait for the current application build and its checks to finish before preparing a checker.')
    return await checker_setup.prepare()


@app.put('/api/provider-settings')
async def save_provider_settings(value: ProviderSettingsBody):
    if architect.start_lock.locked():
        raise HTTPException(409, 'Wait for setup team startup before changing its credentials.')
    async with architect.start_lock:
        public = provider_public_status()
        if public['provider_binding_error']:
            raise HTTPException(409, public['provider_binding_error'])
        if public['work_active'] or architect.runtime is not None:
            raise HTTPException(409, 'Stop the idle setup team before changing credentials. Current work must finish first.')
        if public['locked_provider'] and value.provider != public['locked_provider']:
            raise HTTPException(409, 'This saved setup team uses another provider. Its provider cannot change through credential setup.')
        if any(row['provider'] == value.provider and row.get('source') == 'native_host_config'
                for row in public['providers']):
            raise HTTPException(409, 'This provider uses the selected native model host. Its credentials cannot be replaced here.')
        try:
            provider_settings.save(value.provider, value.api_key)
        except ProviderSettingsUnavailable as error:
            raise HTTPException(409, str(error)) from None
        except ValueError:
            raise HTTPException(422, 'Enter a nonempty provider API key without control characters.') from None
        return provider_public_status()


@app.post('/api/architect/stop')
async def stop_setup_team():
    if architect.start_lock.locked() or provider_owned_work():
        raise HTTPException(409, 'Wait for current setup, animation, build or installation work to finish before stopping the team.')
    async with architect.start_lock:
        # Take the message lock without yielding before rechecking all owners.
        # A guide already waiting for startup owns its message lock and refuses.
        if provider_owned_work():
            raise HTTPException(409, 'The setup team is still in use. Keep its current owner until work finishes.')
        async with architect.message_lock:
            try:
                await architect.stop_for_provider()
            except BaseException:
                raise HTTPException(503, 'Setup team cleanup is unconfirmed. Credentials and restart remain blocked until its owner is resolved.') from None
        return architect.status()


@app.post("/api/architect/context")
async def architect_context(value: BlueprintBody):
    if architect.message_lock.locked() or architect.pending_interaction:
        raise HTTPException(409, "Wait for the current design request before changing its context.")
    blueprint = blueprint_or_error(value.blueprint).model_dump(mode="json")
    architect.set_context(blueprint)
    return {**architect.status(), "hash": fingerprint(blueprint)}


@app.post("/api/architect/start")
async def architect_start(value: BlueprintBody | None = None):
    if value:
        if architect.message_lock.locked() or architect.pending_interaction:
            raise HTTPException(409, "The current design request is still running.")
        blueprint = blueprint_or_error(value.blueprint)
        architect.set_context(blueprint.model_dump(mode="json"))
    try:
        return await architect.start(retry_failed=True)
    except ArchitectUnavailable as error:
        raise HTTPException(503, str(error)) from None
    except Exception as error:
        LOG.warning("Architect start failed (%s)", type(error).__name__)
        raise HTTPException(502, "The setup team could not start. Check the studio logs and provider configuration.") from None


@app.post("/api/architect/message")
async def architect_message(value: ArchitectMessage):
    blueprint = blueprint_or_error(value.blueprint)
    try:
        return await architect.message(value.message, blueprint.model_dump(mode="json"))
    except ArchitectUnavailable as error:
        raise HTTPException(409 if architect.message_lock.locked() or architect.pending_interaction else 503, str(error)) from None
    except TimeoutError as error:
        raise HTTPException(504, str(error)) from None
    except Exception as error:
        LOG.warning("Architect message failed (%s)", type(error).__name__)
        raise HTTPException(502, "The architect could not finish. Open the setup console to inspect its progress before retrying.") from None


@app.get("/api/projects")
async def list_projects():
    return {"projects": await asyncio.to_thread(projects.list)}


@app.get("/api/projects/{slug}/history")
async def project_history(slug: str):
    try:
        return {"revisions": await asyncio.to_thread(projects.history, slug)}
    except ValueError as error:
        raise HTTPException(422, str(error)) from None


@app.get("/api/projects/{slug}")
async def get_project(slug: str, revision: int | None = None):
    try:
        value = await asyncio.to_thread(projects.get, slug, revision)
    except ValueError as error:
        raise HTTPException(422, str(error)) from None
    if value is None:
        raise HTTPException(404, "Project revision not found.")
    return value


@app.put("/api/projects/{slug}")
async def save_project(slug: str, value: ProjectSave):
    spec = blueprint_or_error(value.blueprint)
    if spec.project.slug != slug:
        raise HTTPException(422, "The project slug must match its URL.")
    try:
        return await asyncio.to_thread(projects.save, spec.model_dump(mode="json"), value.expected_revision)
    except DraftConflict as error:
        raise HTTPException(409, str(error)) from None


@app.post("/api/design")
async def design(request: DesignRequest):
    if design_lock.locked():
        raise HTTPException(409, "Your designer is already working. Wait for the current team to finish.")
    async with design_lock:
        try:
            return await design_team(request)
        except Exception as error:
            # Provider errors may include request context. Do not return them or
            # log credentials/prompt text into the studio's HTTP diagnostics.
            LOG.warning("Team design failed (%s)", type(error).__name__)
            raise HTTPException(502, "The AI designer could not finish. Check your provider key and model on the server, then retry or use a starter team.") from None


LEGACY_PACKAGE_RETIRED = ("This legacy package export is retired. Build the agent team in the guided "
    "setup, then use its package or installation controls.")


@app.post("/api/preview")
async def preview(spec: DeploymentSpec):
    raise HTTPException(410, LEGACY_PACKAGE_RETIRED)


@app.post("/api/package")
async def package(spec: DeploymentSpec):
    raise HTTPException(410, LEGACY_PACKAGE_RETIRED)


@app.get("/{path:path}")
async def frontend(path: str):
    root = (APP_ROOT / "web/dist").resolve()
    candidate = (root / path).resolve()
    if not candidate.is_relative_to(root):
        raise HTTPException(404)
    if candidate.is_file():
        return FileResponse(candidate)
    if path.startswith("api/"):
        raise HTTPException(404)
    index = root / "index.html"
    if index.is_file():
        return FileResponse(index)
    return JSONResponse({"detail": "Build the frontend with npm --prefix installer/web run build, or run ./installer/start.sh."}, status_code=503)
