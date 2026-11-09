from __future__ import annotations

import asyncio
import json
import os
import subprocess
from typing import Any, Callable, Literal, Mapping, Protocol, TypedDict, cast
from urllib import request as urllib_request
from urllib.error import HTTPError, URLError


class JsonRpcRequest(TypedDict):
    jsonrpc: Literal["2.0"]
    id: str
    method: str
    params: dict[str, Any]


class JsonRpcSuccess(TypedDict):
    jsonrpc: Literal["2.0"]
    id: str
    result: Any


class JsonRpcErrorBody(TypedDict):
    code: int
    message: str


class JsonRpcErrorResponse(TypedDict):
    jsonrpc: Literal["2.0"]
    id: str
    error: JsonRpcErrorBody


JsonRpcResponse = JsonRpcSuccess | JsonRpcErrorResponse


class MetaModelsCatalogResult(TypedDict):
    models: list[dict[str, Any]]
    provider_defaults: list[dict[str, Any]]


class MetaToolsCatalogResult(TypedDict):
    schema_version: str
    runtime_backed: bool
    source: str
    authoring_provider: dict[str, Any]
    runtime_unavailable_reason: str
    tool_catalog: list[dict[str, Any]]


class MetaSkillsCatalogResult(TypedDict):
    schema_version: str
    runtime_backed: bool
    source: str
    authoring_provider: dict[str, Any]
    runtime_unavailable_reason: str
    skill_realms: list[dict[str, Any]]


class MetaAgentDefinitionsResult(TypedDict):
    schema_version: str
    runtime_backed: bool
    source: str
    authoring_provider: dict[str, Any]
    runtime_unavailable_reason: str
    agent_definitions: list[dict[str, Any]]


class MetaTemplatesResult(TypedDict):
    schema_version: str
    source: str
    authoring_provider: dict[str, Any]
    runtime_unavailable_reason: str
    blank_mobpack: dict[str, Any]
    sample_mobpacks: list[dict[str, Any]]
    sample_agent_definitions: list[dict[str, Any]]
    templates: dict[str, Any]


class MetaCatalogsResult(TypedDict):
    schema_version: str
    runtime_backed: bool
    authoring_provider: dict[str, Any]
    runtime_unavailable_reason: str
    sources: dict[str, Any]
    templates: dict[str, Any]
    tool_catalog: list[dict[str, Any]]
    skill_realms: list[dict[str, Any]]
    blank_mobpack: dict[str, Any]
    sample_mobpacks: list[dict[str, Any]]
    agent_definitions: list[dict[str, Any]]
    sample_agent_definitions: list[dict[str, Any]]
    models: list[dict[str, Any]]
    provider_defaults: list[dict[str, Any]]


class MetaMobpackValidationResult(TypedDict):
    ok: bool
    diagnostics: list[dict[str, Any]]
    display_rows: list[dict[str, Any]]
    flow_ids: list[str]
    validation_source: str
    deploy_command: str


class MetaMobpackSourceResult(TypedDict):
    filename: str
    media_type: str
    mob_toml: str
    source_files: list[dict[str, Any]]
    validation: dict[str, Any]
    source: str


class MetaMobpackExportResult(TypedDict):
    filename: str
    media_type: str
    content_base64: str
    mob_toml: str
    source_files: list[dict[str, Any]]
    validation: dict[str, Any]


class MetaMobpackImportResult(TypedDict):
    document: dict[str, Any]
    validation: dict[str, Any]
    source: str
    source_label: str
    source_media_type: str


class MetaMobpackDraftListResult(TypedDict):
    source: str
    runtime_backed: bool
    rows: list[dict[str, Any]]


class MetaMobpackDraftGetResult(TypedDict):
    source: str
    runtime_backed: bool
    row: dict[str, Any]


class MetaMobpackDraftSaveResult(TypedDict):
    source: str
    row: dict[str, Any]
    rows: list[dict[str, Any]]


class MetaMobpackDraftDeleteResult(TypedDict):
    source: str
    id: str
    deleted: bool
    rows: list[dict[str, Any]]


class MetaMobpackDraftHistoryResult(TypedDict):
    source: str
    stepped: bool
    row: dict[str, Any]
    rows: list[dict[str, Any]]


class MetaMobpackApplyOperationResult(TypedDict):
    source: str
    operation: str
    ok: bool
    document: dict[str, Any]
    selection: dict[str, Any]
    validation: dict[str, Any]


class MetaMobpackDeployCommandResult(TypedDict):
    command: str
    argv: list[str]
    deploy_command: str
    filename: str
    validation: dict[str, Any]
    source: str


class MetaMobpackDeployResult(TypedDict):
    filename: str
    pack_path: str
    pack_sha256: str
    command: str
    argv: list[str]
    plan_trace: list[dict[str, Any]]
    executed: bool
    success: bool
    validation: dict[str, Any]
    display_rows: list[dict[str, Any]]


class MetaStatusResult(TypedDict):
    contract_version: str
    running: bool
    loaded_modules: list[str]


class _MetaCapabilitiesRequired(TypedDict):
    contract_version: str
    methods: list[str]
    loaded_modules: list[str]


class MetaCapabilitiesResult(_MetaCapabilitiesRequired, total=False):
    feature_capabilities: list[str]


class MetaReconcileResult(TypedDict):
    accepted: bool
    reconciled_modules: list[str]
    added: int


class MetaSpawnMemberResult(TypedDict):
    accepted: bool
    module_id: str


class MetaSubscribeKeepAlive(TypedDict):
    interval_ms: int
    event: str


class MetaSubscribeEventEnvelope(TypedDict):
    event_id: str
    source: str
    timestamp_ms: int
    event: Any


class MetaSubscribeResult(TypedDict):
    scope: Literal["mob", "agent", "interaction"]
    replay_from_event_id: str | None
    keep_alive: MetaSubscribeKeepAlive
    keep_alive_comment: str
    event_frames: list[str]
    events: list[MetaSubscribeEventEnvelope]


class MetaSubscribeParams(TypedDict, total=False):
    scope: Literal["mob", "agent", "interaction"]
    last_event_id: str
    agent_id: str


class AsyncRpcTransport(Protocol):
    async def __call__(self, request: JsonRpcRequest) -> Any:
        ...


class SyncRpcTransport(Protocol):
    def __call__(self, request: JsonRpcRequest) -> Any:
        ...


class MetaRpcError(RuntimeError):
    def __init__(self, code: int, message: str, request_id: str, method: str):
        super().__init__(message)
        self.code = code
        self.request_id = request_id
        self.method = method


def create_gateway_sync_transport(gateway_bin: str) -> SyncRpcTransport:
    def transport(request: JsonRpcRequest) -> Any:
        request_json = json.dumps(request)
        proc = subprocess.run(
            [gateway_bin],
            check=False,
            capture_output=True,
            text=True,
            env={**os.environ, "META_RPC_REQUEST": request_json},
        )
        if proc.returncode != 0:
            raise RuntimeError(
                f"gateway failed (status={proc.returncode}): {proc.stderr.strip()}"
            )

        try:
            return json.loads(proc.stdout)
        except json.JSONDecodeError as exc:
            raise ValueError("gateway returned non-JSON response") from exc

    return transport


def create_gateway_async_transport(gateway_bin: str) -> AsyncRpcTransport:
    async def transport(request: JsonRpcRequest) -> Any:
        request_json = json.dumps(request)
        proc = await asyncio.create_subprocess_exec(
            gateway_bin,
            env={**os.environ, "META_RPC_REQUEST": request_json},
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.PIPE,
        )
        stdout, stderr = await proc.communicate()
        if proc.returncode != 0:
            stderr_text = stderr.decode("utf-8", errors="replace").strip()
            raise RuntimeError(
                f"gateway failed (status={proc.returncode}): {stderr_text}"
            )

        try:
            return json.loads(stdout.decode("utf-8"))
        except json.JSONDecodeError as exc:
            raise ValueError("gateway returned non-JSON response") from exc

    return transport


def create_http_transport(
    endpoint: str,
    *,
    headers: Mapping[str, str] | None = None,
    timeout_seconds: float = 10.0,
) -> AsyncRpcTransport:
    base_headers = {"content-type": "application/json", "accept": "application/json"}
    if headers:
        base_headers.update(dict(headers))

    async def transport(request: JsonRpcRequest) -> Any:
        request_bytes = json.dumps(request).encode("utf-8")
        http_request = urllib_request.Request(
            endpoint,
            data=request_bytes,
            method="POST",
            headers=base_headers,
        )
        try:
            body = await asyncio.to_thread(_read_http_body, http_request, timeout_seconds)
        except HTTPError as exc:
            body = exc.read().decode("utf-8", errors="replace")
            raise RuntimeError(
                f"http transport failed (status={exc.code}): {body}"
            ) from exc
        except URLError as exc:
            raise RuntimeError(f"http transport failed: {exc.reason}") from exc

        try:
            return json.loads(body)
        except json.JSONDecodeError as exc:
            raise ValueError("http transport returned non-JSON response") from exc

    return transport


class MetaTypedClient:
    def __init__(self, gateway_bin: str):
        self.gateway_bin = gateway_bin
        self._sync_transport = create_gateway_sync_transport(gateway_bin)

    @classmethod
    def from_persistent(cls, transport: SyncRpcTransport) -> "MetaTypedClient":
        instance = cls.__new__(cls)
        instance.gateway_bin = ""
        instance._sync_transport = transport
        return instance

    def rpc(
        self, request_id: str, method: str, params: Mapping[str, Any] | None = None
    ) -> JsonRpcResponse:
        payload = self._sync_transport(_build_request(request_id, method, params))
        return _parse_json_rpc_response(payload, request_id)

    def status(self, request_id: str = "status") -> MetaStatusResult:
        return cast(
            MetaStatusResult,
            _unwrap_typed_result(
                self.rpc(request_id, "meta/status", {}),
                request_id,
                "meta/status",
                _is_status_result,
            ),
        )

    def capabilities(self, request_id: str = "capabilities") -> MetaCapabilitiesResult:
        return cast(
            MetaCapabilitiesResult,
            _unwrap_typed_result(
                self.rpc(request_id, "meta/capabilities", {}),
                request_id,
                "meta/capabilities",
                _is_capabilities_result,
            ),
        )

    def reconcile(
        self, modules: list[str], request_id: str = "reconcile"
    ) -> MetaReconcileResult:
        return cast(
            MetaReconcileResult,
            _unwrap_typed_result(
                self.rpc(request_id, "meta/reconcile", {"modules": modules}),
                request_id,
                "meta/reconcile",
                _is_reconcile_result,
            ),
        )

    def spawn_member(
        self, module_id: str, request_id: str = "spawn_member"
    ) -> MetaSpawnMemberResult:
        return cast(
            MetaSpawnMemberResult,
            _unwrap_typed_result(
                self.rpc(request_id, "meta/spawn_member", {"module_id": module_id}),
                request_id,
                "meta/spawn_member",
                _is_spawn_member_result,
            ),
        )

    def subscribe_events(
        self,
        params: MetaSubscribeParams | None = None,
        request_id: str = "events_subscribe",
    ) -> MetaSubscribeResult:
        return cast(
            MetaSubscribeResult,
            _unwrap_typed_result(
                self.rpc(
                    request_id,
                    "meta/events/subscribe",
                    dict(params) if params is not None else {},
                ),
                request_id,
                "meta/events/subscribe",
                _is_subscribe_result,
            ),
        )

    def models_catalog(
        self, request_id: str = "models_catalog"
    ) -> MetaModelsCatalogResult:
        return cast(
            MetaModelsCatalogResult,
            _unwrap_typed_result(
                self.rpc(request_id, "meta/models/catalog", {}),
                request_id,
                "meta/models/catalog",
                _is_models_catalog_result,
            ),
        )

    def tools_catalog(
        self, request_id: str = "tools_catalog"
    ) -> MetaToolsCatalogResult:
        return cast(
            MetaToolsCatalogResult,
            _unwrap_typed_result(
                self.rpc(request_id, "meta/tools/catalog", {}),
                request_id,
                "meta/tools/catalog",
                _is_tools_catalog_result,
            ),
        )

    def skills_catalog(
        self, request_id: str = "skills_catalog"
    ) -> MetaSkillsCatalogResult:
        return cast(
            MetaSkillsCatalogResult,
            _unwrap_typed_result(
                self.rpc(request_id, "meta/skills/catalog", {}),
                request_id,
                "meta/skills/catalog",
                _is_skills_catalog_result,
            ),
        )

    def agent_definitions(
        self, request_id: str = "agent_definitions"
    ) -> MetaAgentDefinitionsResult:
        return cast(
            MetaAgentDefinitionsResult,
            _unwrap_typed_result(
                self.rpc(request_id, "meta/agent_definitions/list", {}),
                request_id,
                "meta/agent_definitions/list",
                _is_agent_definitions_result,
            ),
        )

    def mobpack_templates(
        self, request_id: str = "mobpack_templates"
    ) -> MetaTemplatesResult:
        return cast(
            MetaTemplatesResult,
            _unwrap_typed_result(
                self.rpc(request_id, "meta/mobpacks/templates", {}),
                request_id,
                "meta/mobpacks/templates",
                _is_mobpack_templates_result,
            ),
        )

    def mobpack_catalogs(
        self, request_id: str = "mobpack_catalogs"
    ) -> MetaCatalogsResult:
        return cast(
            MetaCatalogsResult,
            _unwrap_typed_result(
                self.rpc(request_id, "meta/mobpacks/catalogs", {}),
                request_id,
                "meta/mobpacks/catalogs",
                _is_mobpack_catalogs_result,
            ),
        )

    def mobpack_validate(
        self,
        document: Mapping[str, Any],
        *,
        rkat_validate: bool | None = None,
        request_id: str = "mobpack_validate",
    ) -> MetaMobpackValidationResult:
        return cast(
            MetaMobpackValidationResult,
            _unwrap_typed_result(
                self.rpc(
                    request_id,
                    "meta/mobpacks/validate",
                    _mobpack_validate_params(document, rkat_validate),
                ),
                request_id,
                "meta/mobpacks/validate",
                _is_mobpack_validation_result,
            ),
        )

    def mobpack_source(
        self,
        document: Mapping[str, Any],
        request_id: str = "mobpack_source",
    ) -> MetaMobpackSourceResult:
        return cast(
            MetaMobpackSourceResult,
            _unwrap_typed_result(
                self.rpc(
                    request_id, "meta/mobpacks/source", {"document": dict(document)}
                ),
                request_id,
                "meta/mobpacks/source",
                _is_mobpack_source_result,
            ),
        )

    def mobpack_export(
        self,
        document: Mapping[str, Any],
        request_id: str = "mobpack_export",
    ) -> MetaMobpackExportResult:
        return cast(
            MetaMobpackExportResult,
            _unwrap_typed_result(
                self.rpc(
                    request_id, "meta/mobpacks/export", {"document": dict(document)}
                ),
                request_id,
                "meta/mobpacks/export",
                _is_mobpack_export_result,
            ),
        )

    def mobpack_import(
        self,
        *,
        mob_toml: str | None = None,
        content_base64: str | None = None,
        document: Mapping[str, Any] | None = None,
        source_name: str | None = None,
        request_id: str = "mobpack_import",
    ) -> MetaMobpackImportResult:
        return cast(
            MetaMobpackImportResult,
            _unwrap_typed_result(
                self.rpc(
                    request_id,
                    "meta/mobpacks/import",
                    _mobpack_import_params(
                        mob_toml, content_base64, document, source_name
                    ),
                ),
                request_id,
                "meta/mobpacks/import",
                _is_mobpack_import_result,
            ),
        )

    def mobpack_list(
        self, request_id: str = "mobpack_list"
    ) -> MetaMobpackDraftListResult:
        return cast(
            MetaMobpackDraftListResult,
            _unwrap_typed_result(
                self.rpc(request_id, "meta/mobpacks/list", {}),
                request_id,
                "meta/mobpacks/list",
                _is_mobpack_draft_list_result,
            ),
        )

    def mobpack_get(
        self, draft_id: str, request_id: str = "mobpack_get"
    ) -> MetaMobpackDraftGetResult:
        return cast(
            MetaMobpackDraftGetResult,
            _unwrap_typed_result(
                self.rpc(request_id, "meta/mobpacks/get", {"id": draft_id}),
                request_id,
                "meta/mobpacks/get",
                _is_mobpack_draft_get_result,
            ),
        )

    def mobpack_create(
        self,
        *,
        template: str | None = None,
        name: str | None = None,
        trigger: str | None = None,
        request_id: str = "mobpack_create",
    ) -> MetaMobpackDraftSaveResult:
        return cast(
            MetaMobpackDraftSaveResult,
            _unwrap_typed_result(
                self.rpc(
                    request_id,
                    "meta/mobpacks/create",
                    _mobpack_create_params(template, name, trigger),
                ),
                request_id,
                "meta/mobpacks/create",
                _is_mobpack_draft_save_result,
            ),
        )

    def mobpack_save(
        self,
        draft_id: str,
        document: Mapping[str, Any],
        *,
        validation: Mapping[str, Any] | None = None,
        stage: str | None = None,
        expected_revision: int | None = None,
        expected_etag: str | None = None,
        request_id: str = "mobpack_save",
    ) -> MetaMobpackDraftSaveResult:
        return cast(
            MetaMobpackDraftSaveResult,
            _unwrap_typed_result(
                self.rpc(
                    request_id,
                    "meta/mobpacks/save",
                    _mobpack_save_params(
                        draft_id,
                        document,
                        validation,
                        stage,
                        expected_revision,
                        expected_etag,
                    ),
                ),
                request_id,
                "meta/mobpacks/save",
                _is_mobpack_draft_save_result,
            ),
        )

    def mobpack_delete(
        self,
        draft_id: str,
        *,
        expected_revision: int | None = None,
        request_id: str = "mobpack_delete",
    ) -> MetaMobpackDraftDeleteResult:
        return cast(
            MetaMobpackDraftDeleteResult,
            _unwrap_typed_result(
                self.rpc(
                    request_id,
                    "meta/mobpacks/delete",
                    _mobpack_delete_params(draft_id, expected_revision),
                ),
                request_id,
                "meta/mobpacks/delete",
                _is_mobpack_draft_delete_result,
            ),
        )

    def mobpack_undo(
        self,
        draft_id: str,
        *,
        expected_revision: int | None = None,
        expected_etag: str | None = None,
        request_id: str = "mobpack_undo",
    ) -> MetaMobpackDraftHistoryResult:
        return cast(
            MetaMobpackDraftHistoryResult,
            _unwrap_typed_result(
                self.rpc(
                    request_id,
                    "meta/mobpacks/undo",
                    _mobpack_history_params(
                        draft_id, expected_revision, expected_etag
                    ),
                ),
                request_id,
                "meta/mobpacks/undo",
                _is_mobpack_draft_history_result,
            ),
        )

    def mobpack_redo(
        self,
        draft_id: str,
        *,
        expected_revision: int | None = None,
        expected_etag: str | None = None,
        request_id: str = "mobpack_redo",
    ) -> MetaMobpackDraftHistoryResult:
        return cast(
            MetaMobpackDraftHistoryResult,
            _unwrap_typed_result(
                self.rpc(
                    request_id,
                    "meta/mobpacks/redo",
                    _mobpack_history_params(
                        draft_id, expected_revision, expected_etag
                    ),
                ),
                request_id,
                "meta/mobpacks/redo",
                _is_mobpack_draft_history_result,
            ),
        )

    def mobpack_apply_operation(
        self,
        document: Mapping[str, Any],
        operation: Mapping[str, Any],
        *,
        expected_catalog_snapshot_id: str | None = None,
        request_id: str = "mobpack_apply_operation",
    ) -> MetaMobpackApplyOperationResult:
        return cast(
            MetaMobpackApplyOperationResult,
            _unwrap_typed_result(
                self.rpc(
                    request_id,
                    "meta/mobpacks/apply_operation",
                    _mobpack_apply_operation_params(
                        document, operation, expected_catalog_snapshot_id
                    ),
                ),
                request_id,
                "meta/mobpacks/apply_operation",
                _is_mobpack_apply_operation_result,
            ),
        )

    def mobpack_deploy_command(
        self,
        document: Mapping[str, Any],
        request_id: str = "mobpack_deploy_command",
    ) -> MetaMobpackDeployCommandResult:
        return cast(
            MetaMobpackDeployCommandResult,
            _unwrap_typed_result(
                self.rpc(
                    request_id,
                    "meta/mobpacks/deploy_command",
                    {"document": dict(document)},
                ),
                request_id,
                "meta/mobpacks/deploy_command",
                _is_mobpack_deploy_command_result,
            ),
        )

    def mobpack_deploy(
        self,
        document: Mapping[str, Any],
        *,
        execute: bool | None = None,
        request_id: str = "mobpack_deploy",
    ) -> MetaMobpackDeployResult:
        return cast(
            MetaMobpackDeployResult,
            _unwrap_typed_result(
                self.rpc(
                    request_id,
                    "meta/mobpacks/deploy",
                    _mobpack_deploy_params(document, execute),
                ),
                request_id,
                "meta/mobpacks/deploy",
                _is_mobpack_deploy_result,
            ),
        )


class MetaAsyncTypedClient:
    def __init__(self, transport: AsyncRpcTransport):
        self._transport = transport

    @classmethod
    def from_gateway_bin(cls, gateway_bin: str) -> "MetaAsyncTypedClient":
        return cls(create_gateway_async_transport(gateway_bin))

    @classmethod
    def from_http(
        cls,
        endpoint: str,
        *,
        headers: Mapping[str, str] | None = None,
        timeout_seconds: float = 10.0,
    ) -> "MetaAsyncTypedClient":
        return cls(
            create_http_transport(
                endpoint,
                headers=headers,
                timeout_seconds=timeout_seconds,
            )
        )

    async def rpc(
        self, request_id: str, method: str, params: Mapping[str, Any] | None = None
    ) -> JsonRpcResponse:
        payload = await self._transport(_build_request(request_id, method, params))
        return _parse_json_rpc_response(payload, request_id)

    async def request(
        self,
        request_id: str,
        method: str,
        params: Mapping[str, Any] | None,
        validator: Callable[[Any], bool],
    ) -> Any:
        response = await self.rpc(request_id, method, params)
        return _unwrap_typed_result(response, request_id, method, validator)

    async def status(self, request_id: str = "status") -> MetaStatusResult:
        return cast(
            MetaStatusResult,
            await self.request(request_id, "meta/status", {}, _is_status_result),
        )

    async def capabilities(
        self, request_id: str = "capabilities"
    ) -> MetaCapabilitiesResult:
        return cast(
            MetaCapabilitiesResult,
            await self.request(
                request_id,
                "meta/capabilities",
                {},
                _is_capabilities_result,
            ),
        )

    async def reconcile(
        self, modules: list[str], request_id: str = "reconcile"
    ) -> MetaReconcileResult:
        return cast(
            MetaReconcileResult,
            await self.request(
                request_id,
                "meta/reconcile",
                {"modules": modules},
                _is_reconcile_result,
            ),
        )

    async def spawn_member(
        self, module_id: str, request_id: str = "spawn_member"
    ) -> MetaSpawnMemberResult:
        return cast(
            MetaSpawnMemberResult,
            await self.request(
                request_id,
                "meta/spawn_member",
                {"module_id": module_id},
                _is_spawn_member_result,
            ),
        )

    async def subscribe_events(
        self,
        params: MetaSubscribeParams | None = None,
        request_id: str = "events_subscribe",
    ) -> MetaSubscribeResult:
        return cast(
            MetaSubscribeResult,
            await self.request(
                request_id,
                "meta/events/subscribe",
                dict(params) if params is not None else {},
                _is_subscribe_result,
            ),
        )

    async def models_catalog(
        self, request_id: str = "models_catalog"
    ) -> MetaModelsCatalogResult:
        return cast(
            MetaModelsCatalogResult,
            await self.request(
                request_id,
                "meta/models/catalog",
                {},
                _is_models_catalog_result,
            ),
        )

    async def tools_catalog(
        self, request_id: str = "tools_catalog"
    ) -> MetaToolsCatalogResult:
        return cast(
            MetaToolsCatalogResult,
            await self.request(
                request_id,
                "meta/tools/catalog",
                {},
                _is_tools_catalog_result,
            ),
        )

    async def skills_catalog(
        self, request_id: str = "skills_catalog"
    ) -> MetaSkillsCatalogResult:
        return cast(
            MetaSkillsCatalogResult,
            await self.request(
                request_id,
                "meta/skills/catalog",
                {},
                _is_skills_catalog_result,
            ),
        )

    async def agent_definitions(
        self, request_id: str = "agent_definitions"
    ) -> MetaAgentDefinitionsResult:
        return cast(
            MetaAgentDefinitionsResult,
            await self.request(
                request_id,
                "meta/agent_definitions/list",
                {},
                _is_agent_definitions_result,
            ),
        )

    async def mobpack_templates(
        self, request_id: str = "mobpack_templates"
    ) -> MetaTemplatesResult:
        return cast(
            MetaTemplatesResult,
            await self.request(
                request_id,
                "meta/mobpacks/templates",
                {},
                _is_mobpack_templates_result,
            ),
        )

    async def mobpack_catalogs(
        self, request_id: str = "mobpack_catalogs"
    ) -> MetaCatalogsResult:
        return cast(
            MetaCatalogsResult,
            await self.request(
                request_id,
                "meta/mobpacks/catalogs",
                {},
                _is_mobpack_catalogs_result,
            ),
        )

    async def mobpack_validate(
        self,
        document: Mapping[str, Any],
        *,
        rkat_validate: bool | None = None,
        request_id: str = "mobpack_validate",
    ) -> MetaMobpackValidationResult:
        return cast(
            MetaMobpackValidationResult,
            await self.request(
                request_id,
                "meta/mobpacks/validate",
                _mobpack_validate_params(document, rkat_validate),
                _is_mobpack_validation_result,
            ),
        )

    async def mobpack_source(
        self,
        document: Mapping[str, Any],
        request_id: str = "mobpack_source",
    ) -> MetaMobpackSourceResult:
        return cast(
            MetaMobpackSourceResult,
            await self.request(
                request_id,
                "meta/mobpacks/source",
                {"document": dict(document)},
                _is_mobpack_source_result,
            ),
        )

    async def mobpack_export(
        self,
        document: Mapping[str, Any],
        request_id: str = "mobpack_export",
    ) -> MetaMobpackExportResult:
        return cast(
            MetaMobpackExportResult,
            await self.request(
                request_id,
                "meta/mobpacks/export",
                {"document": dict(document)},
                _is_mobpack_export_result,
            ),
        )

    async def mobpack_import(
        self,
        *,
        mob_toml: str | None = None,
        content_base64: str | None = None,
        document: Mapping[str, Any] | None = None,
        source_name: str | None = None,
        request_id: str = "mobpack_import",
    ) -> MetaMobpackImportResult:
        return cast(
            MetaMobpackImportResult,
            await self.request(
                request_id,
                "meta/mobpacks/import",
                _mobpack_import_params(mob_toml, content_base64, document, source_name),
                _is_mobpack_import_result,
            ),
        )

    async def mobpack_list(
        self, request_id: str = "mobpack_list"
    ) -> MetaMobpackDraftListResult:
        return cast(
            MetaMobpackDraftListResult,
            await self.request(
                request_id,
                "meta/mobpacks/list",
                {},
                _is_mobpack_draft_list_result,
            ),
        )

    async def mobpack_get(
        self, draft_id: str, request_id: str = "mobpack_get"
    ) -> MetaMobpackDraftGetResult:
        return cast(
            MetaMobpackDraftGetResult,
            await self.request(
                request_id,
                "meta/mobpacks/get",
                {"id": draft_id},
                _is_mobpack_draft_get_result,
            ),
        )

    async def mobpack_create(
        self,
        *,
        template: str | None = None,
        name: str | None = None,
        trigger: str | None = None,
        request_id: str = "mobpack_create",
    ) -> MetaMobpackDraftSaveResult:
        return cast(
            MetaMobpackDraftSaveResult,
            await self.request(
                request_id,
                "meta/mobpacks/create",
                _mobpack_create_params(template, name, trigger),
                _is_mobpack_draft_save_result,
            ),
        )

    async def mobpack_save(
        self,
        draft_id: str,
        document: Mapping[str, Any],
        *,
        validation: Mapping[str, Any] | None = None,
        stage: str | None = None,
        expected_revision: int | None = None,
        expected_etag: str | None = None,
        request_id: str = "mobpack_save",
    ) -> MetaMobpackDraftSaveResult:
        return cast(
            MetaMobpackDraftSaveResult,
            await self.request(
                request_id,
                "meta/mobpacks/save",
                _mobpack_save_params(
                    draft_id,
                    document,
                    validation,
                    stage,
                    expected_revision,
                    expected_etag,
                ),
                _is_mobpack_draft_save_result,
            ),
        )

    async def mobpack_delete(
        self,
        draft_id: str,
        *,
        expected_revision: int | None = None,
        request_id: str = "mobpack_delete",
    ) -> MetaMobpackDraftDeleteResult:
        return cast(
            MetaMobpackDraftDeleteResult,
            await self.request(
                request_id,
                "meta/mobpacks/delete",
                _mobpack_delete_params(draft_id, expected_revision),
                _is_mobpack_draft_delete_result,
            ),
        )

    async def mobpack_undo(
        self,
        draft_id: str,
        *,
        expected_revision: int | None = None,
        expected_etag: str | None = None,
        request_id: str = "mobpack_undo",
    ) -> MetaMobpackDraftHistoryResult:
        return cast(
            MetaMobpackDraftHistoryResult,
            await self.request(
                request_id,
                "meta/mobpacks/undo",
                _mobpack_history_params(draft_id, expected_revision, expected_etag),
                _is_mobpack_draft_history_result,
            ),
        )

    async def mobpack_redo(
        self,
        draft_id: str,
        *,
        expected_revision: int | None = None,
        expected_etag: str | None = None,
        request_id: str = "mobpack_redo",
    ) -> MetaMobpackDraftHistoryResult:
        return cast(
            MetaMobpackDraftHistoryResult,
            await self.request(
                request_id,
                "meta/mobpacks/redo",
                _mobpack_history_params(draft_id, expected_revision, expected_etag),
                _is_mobpack_draft_history_result,
            ),
        )

    async def mobpack_apply_operation(
        self,
        document: Mapping[str, Any],
        operation: Mapping[str, Any],
        *,
        expected_catalog_snapshot_id: str | None = None,
        request_id: str = "mobpack_apply_operation",
    ) -> MetaMobpackApplyOperationResult:
        return cast(
            MetaMobpackApplyOperationResult,
            await self.request(
                request_id,
                "meta/mobpacks/apply_operation",
                _mobpack_apply_operation_params(
                    document, operation, expected_catalog_snapshot_id
                ),
                _is_mobpack_apply_operation_result,
            ),
        )

    async def mobpack_deploy_command(
        self,
        document: Mapping[str, Any],
        request_id: str = "mobpack_deploy_command",
    ) -> MetaMobpackDeployCommandResult:
        return cast(
            MetaMobpackDeployCommandResult,
            await self.request(
                request_id,
                "meta/mobpacks/deploy_command",
                {"document": dict(document)},
                _is_mobpack_deploy_command_result,
            ),
        )

    async def mobpack_deploy(
        self,
        document: Mapping[str, Any],
        *,
        execute: bool | None = None,
        request_id: str = "mobpack_deploy",
    ) -> MetaMobpackDeployResult:
        return cast(
            MetaMobpackDeployResult,
            await self.request(
                request_id,
                "meta/mobpacks/deploy",
                _mobpack_deploy_params(document, execute),
                _is_mobpack_deploy_result,
            ),
        )


def _read_http_body(http_request: urllib_request.Request, timeout_seconds: float) -> str:
    with urllib_request.urlopen(http_request, timeout=timeout_seconds) as response:
        return response.read().decode("utf-8")


def _build_request(
    request_id: str,
    method: str,
    params: Mapping[str, Any] | None,
) -> JsonRpcRequest:
    return {
        "jsonrpc": "2.0",
        "id": request_id,
        "method": method,
        "params": dict(params) if params is not None else {},
    }


def _mobpack_validate_params(
    document: Mapping[str, Any], rkat_validate: bool | None
) -> dict[str, Any]:
    params: dict[str, Any] = {"document": dict(document)}
    if rkat_validate is not None:
        params["rkat_validate"] = rkat_validate
    return params


def _mobpack_import_params(
    mob_toml: str | None,
    content_base64: str | None,
    document: Mapping[str, Any] | None,
    source_name: str | None,
) -> dict[str, Any]:
    params: dict[str, Any] = {}
    if mob_toml is not None:
        params["mob_toml"] = mob_toml
    if content_base64 is not None:
        params["content_base64"] = content_base64
    if document is not None:
        params["document"] = dict(document)
    if source_name is not None:
        params["source_name"] = source_name
    return params


def _mobpack_create_params(
    template: str | None, name: str | None, trigger: str | None
) -> dict[str, Any]:
    params: dict[str, Any] = {}
    if template is not None:
        params["template"] = template
    if name is not None:
        params["name"] = name
    if trigger is not None:
        params["trigger"] = trigger
    return params


def _mobpack_save_params(
    draft_id: str,
    document: Mapping[str, Any],
    validation: Mapping[str, Any] | None,
    stage: str | None,
    expected_revision: int | None,
    expected_etag: str | None,
) -> dict[str, Any]:
    params: dict[str, Any] = {"id": draft_id, "document": dict(document)}
    if validation is not None:
        params["validation"] = dict(validation)
    if stage is not None:
        params["stage"] = stage
    if expected_revision is not None:
        params["expected_revision"] = expected_revision
    if expected_etag is not None:
        params["expected_etag"] = expected_etag
    return params


def _mobpack_history_params(
    draft_id: str,
    expected_revision: int | None,
    expected_etag: str | None,
) -> dict[str, Any]:
    params: dict[str, Any] = {"id": draft_id}
    if expected_revision is not None:
        params["expected_revision"] = expected_revision
    if expected_etag is not None:
        params["expected_etag"] = expected_etag
    return params


def _mobpack_delete_params(
    draft_id: str, expected_revision: int | None
) -> dict[str, Any]:
    params: dict[str, Any] = {"id": draft_id}
    if expected_revision is not None:
        params["expected_revision"] = expected_revision
    return params


def _mobpack_apply_operation_params(
    document: Mapping[str, Any],
    operation: Mapping[str, Any],
    expected_catalog_snapshot_id: str | None,
) -> dict[str, Any]:
    params: dict[str, Any] = {
        "document": dict(document),
        "operation": dict(operation),
    }
    if expected_catalog_snapshot_id is not None:
        params["expected_catalog_snapshot_id"] = expected_catalog_snapshot_id
    return params


def _mobpack_deploy_params(
    document: Mapping[str, Any], execute: bool | None
) -> dict[str, Any]:
    params: dict[str, Any] = {"document": dict(document)}
    if execute is not None:
        params["execute"] = execute
    return params


def _parse_json_rpc_response(payload: Any, request_id: str) -> JsonRpcResponse:
    if not isinstance(payload, dict):
        raise ValueError("invalid JSON-RPC response envelope")
    if payload.get("jsonrpc") != "2.0" or payload.get("id") != request_id:
        raise ValueError("invalid JSON-RPC response envelope")

    has_result = "result" in payload
    has_error = "error" in payload
    if has_result == has_error:
        raise ValueError("invalid JSON-RPC response envelope")

    if has_error:
        error = payload.get("error")
        if not isinstance(error, dict):
            raise ValueError("invalid JSON-RPC response envelope")
        code = error.get("code")
        message = error.get("message")
        if not isinstance(code, int) or isinstance(code, bool):
            raise ValueError("invalid JSON-RPC response envelope")
        if not isinstance(message, str):
            raise ValueError("invalid JSON-RPC response envelope")

    return cast(JsonRpcResponse, payload)


def _unwrap_typed_result(
    response: JsonRpcResponse,
    request_id: str,
    method: str,
    validator: Callable[[Any], bool],
) -> Any:
    if "error" in response:
        error = response["error"]
        raise MetaRpcError(error["code"], error["message"], request_id, method)

    result = response["result"]
    if not validator(result):
        raise ValueError(f"invalid result payload for {method}")
    return result


def _is_status_result(value: Any) -> bool:
    return (
        isinstance(value, dict)
        and isinstance(value.get("contract_version"), str)
        and isinstance(value.get("running"), bool)
        and _is_string_list(value.get("loaded_modules"))
    )


def _is_capabilities_result(value: Any) -> bool:
    return (
        isinstance(value, dict)
        and isinstance(value.get("contract_version"), str)
        and (
            "feature_capabilities" not in value
            or _is_string_list(value.get("feature_capabilities"))
        )
        and _is_string_list(value.get("methods"))
        and _is_string_list(value.get("loaded_modules"))
    )


def _is_reconcile_result(value: Any) -> bool:
    return (
        isinstance(value, dict)
        and isinstance(value.get("accepted"), bool)
        and _is_string_list(value.get("reconciled_modules"))
        and isinstance(value.get("added"), int)
        and not isinstance(value.get("added"), bool)
    )


def _is_spawn_member_result(value: Any) -> bool:
    return (
        isinstance(value, dict)
        and isinstance(value.get("accepted"), bool)
        and isinstance(value.get("module_id"), str)
    )


def _is_subscribe_result(value: Any) -> bool:
    if not isinstance(value, dict):
        return False

    scope = value.get("scope")
    if scope not in {"mob", "agent", "interaction"}:
        return False

    replay = value.get("replay_from_event_id")
    if replay is not None and not isinstance(replay, str):
        return False

    keep_alive = value.get("keep_alive")
    if not isinstance(keep_alive, dict):
        return False

    interval = keep_alive.get("interval_ms")
    if not isinstance(interval, int) or isinstance(interval, bool):
        return False
    if not isinstance(keep_alive.get("event"), str):
        return False

    if not isinstance(value.get("keep_alive_comment"), str):
        return False

    if not _is_string_list(value.get("event_frames")):
        return False

    events = value.get("events")
    if not isinstance(events, list):
        return False

    for event in events:
        if not isinstance(event, dict):
            return False
        timestamp = event.get("timestamp_ms")
        if (
            not isinstance(event.get("event_id"), str)
            or not isinstance(event.get("source"), str)
            or not isinstance(timestamp, int)
            or isinstance(timestamp, bool)
            or "event" not in event
        ):
            return False

    return True


def _is_models_catalog_result(value: Any) -> bool:
    return (
        isinstance(value, dict)
        and isinstance(value.get("models"), list)
        and isinstance(value.get("provider_defaults"), list)
    )


def _is_tools_catalog_result(value: Any) -> bool:
    return (
        isinstance(value, dict)
        and isinstance(value.get("schema_version"), str)
        and isinstance(value.get("runtime_backed"), bool)
        and isinstance(value.get("source"), str)
        and isinstance(value.get("authoring_provider"), dict)
        and isinstance(value.get("tool_catalog"), list)
    )


def _is_skills_catalog_result(value: Any) -> bool:
    return (
        isinstance(value, dict)
        and isinstance(value.get("schema_version"), str)
        and isinstance(value.get("runtime_backed"), bool)
        and isinstance(value.get("source"), str)
        and isinstance(value.get("authoring_provider"), dict)
        and isinstance(value.get("skill_realms"), list)
    )


def _is_agent_definitions_result(value: Any) -> bool:
    return (
        isinstance(value, dict)
        and isinstance(value.get("schema_version"), str)
        and isinstance(value.get("runtime_backed"), bool)
        and isinstance(value.get("source"), str)
        and isinstance(value.get("authoring_provider"), dict)
        and isinstance(value.get("agent_definitions"), list)
    )


def _is_mobpack_templates_result(value: Any) -> bool:
    return (
        isinstance(value, dict)
        and isinstance(value.get("schema_version"), str)
        and isinstance(value.get("source"), str)
        and isinstance(value.get("authoring_provider"), dict)
        and isinstance(value.get("blank_mobpack"), dict)
        and isinstance(value.get("sample_mobpacks"), list)
        and isinstance(value.get("sample_agent_definitions"), list)
        and isinstance(value.get("templates"), dict)
    )


def _is_mobpack_catalogs_result(value: Any) -> bool:
    return (
        isinstance(value, dict)
        and isinstance(value.get("schema_version"), str)
        and isinstance(value.get("runtime_backed"), bool)
        and isinstance(value.get("authoring_provider"), dict)
        and isinstance(value.get("sources"), dict)
        and isinstance(value.get("templates"), dict)
        and isinstance(value.get("tool_catalog"), list)
        and isinstance(value.get("skill_realms"), list)
        and isinstance(value.get("blank_mobpack"), dict)
        and isinstance(value.get("sample_mobpacks"), list)
        and isinstance(value.get("agent_definitions"), list)
        and isinstance(value.get("sample_agent_definitions"), list)
        and isinstance(value.get("models"), list)
        and isinstance(value.get("provider_defaults"), list)
    )


def _is_mobpack_validation_result(value: Any) -> bool:
    return (
        isinstance(value, dict)
        and isinstance(value.get("ok"), bool)
        and isinstance(value.get("diagnostics"), list)
        and isinstance(value.get("display_rows"), list)
        and isinstance(value.get("deploy_command"), str)
    )


def _is_mobpack_source_result(value: Any) -> bool:
    return (
        isinstance(value, dict)
        and isinstance(value.get("filename"), str)
        and isinstance(value.get("mob_toml"), str)
        and isinstance(value.get("source_files"), list)
        and isinstance(value.get("validation"), dict)
    )


def _is_mobpack_export_result(value: Any) -> bool:
    return (
        isinstance(value, dict)
        and isinstance(value.get("filename"), str)
        and isinstance(value.get("media_type"), str)
        and isinstance(value.get("content_base64"), str)
        and isinstance(value.get("validation"), dict)
    )


def _is_mobpack_import_result(value: Any) -> bool:
    return (
        isinstance(value, dict)
        and isinstance(value.get("document"), dict)
        and isinstance(value.get("validation"), dict)
        and isinstance(value.get("source"), str)
    )


def _is_mobpack_draft_list_result(value: Any) -> bool:
    return (
        isinstance(value, dict)
        and isinstance(value.get("source"), str)
        and isinstance(value.get("rows"), list)
    )


def _is_mobpack_draft_get_result(value: Any) -> bool:
    return (
        isinstance(value, dict)
        and isinstance(value.get("source"), str)
        and isinstance(value.get("row"), dict)
    )


def _is_mobpack_draft_save_result(value: Any) -> bool:
    return (
        isinstance(value, dict)
        and isinstance(value.get("source"), str)
        and isinstance(value.get("row"), dict)
        and isinstance(value.get("rows"), list)
    )


def _is_mobpack_draft_delete_result(value: Any) -> bool:
    return (
        isinstance(value, dict)
        and isinstance(value.get("source"), str)
        and isinstance(value.get("id"), str)
        and isinstance(value.get("deleted"), bool)
        and isinstance(value.get("rows"), list)
    )


def _is_mobpack_draft_history_result(value: Any) -> bool:
    return (
        isinstance(value, dict)
        and isinstance(value.get("source"), str)
        and isinstance(value.get("stepped"), bool)
        and isinstance(value.get("row"), dict)
        and isinstance(value.get("rows"), list)
    )


def _is_mobpack_apply_operation_result(value: Any) -> bool:
    return (
        isinstance(value, dict)
        and isinstance(value.get("ok"), bool)
        and isinstance(value.get("operation"), str)
        and isinstance(value.get("document"), dict)
        and isinstance(value.get("validation"), dict)
    )


def _is_mobpack_deploy_command_result(value: Any) -> bool:
    return (
        isinstance(value, dict)
        and isinstance(value.get("command"), str)
        and isinstance(value.get("argv"), list)
        and isinstance(value.get("deploy_command"), str)
        and isinstance(value.get("validation"), dict)
    )


def _is_mobpack_deploy_result(value: Any) -> bool:
    return (
        isinstance(value, dict)
        and isinstance(value.get("filename"), str)
        and isinstance(value.get("command"), str)
        and isinstance(value.get("executed"), bool)
        and isinstance(value.get("success"), bool)
        and isinstance(value.get("validation"), dict)
    )


def _is_string_list(value: Any) -> bool:
    return isinstance(value, list) and all(isinstance(item, str) for item in value)
