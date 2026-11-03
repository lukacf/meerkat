from __future__ import annotations

from dataclasses import dataclass, field
from typing import Any, Awaitable, Callable, Literal, Mapping, Sequence
from urllib.parse import quote

RestartPolicy = Literal["never", "always", "on_failure"]
ModuleBoundary = Literal["mcp"]


def build_console_route(
    path: Literal["/console/modules", "/console/experience"],
    auth_token: str | None = None,
) -> str:
    if not auth_token:
        return path
    joiner = "&" if "?" in path else "?"
    return f"{path}{joiner}auth_token={quote(auth_token, safe='')}"


def build_console_modules_route(auth_token: str | None = None) -> str:
    return build_console_route("/console/modules", auth_token)


def build_console_experience_route(auth_token: str | None = None) -> str:
    return build_console_route("/console/experience", auth_token)


def build_console_routes(auth_token: str | None = None) -> dict[str, str]:
    return {
        "modules": build_console_modules_route(auth_token),
        "experience": build_console_experience_route(auth_token),
    }


@dataclass(frozen=True)
class ModuleSpec:
    id: str
    command: str
    args: tuple[str, ...]
    restart_policy: RestartPolicy = "never"
    boundary: ModuleBoundary | None = None
    env: dict[str, str] = field(default_factory=dict)

    def to_dict(self) -> dict[str, object]:
        payload: dict[str, object] = {
            "id": self.id,
            "command": self.command,
            "args": list(self.args),
            "restart_policy": self.restart_policy,
        }
        if self.boundary is not None:
            payload["boundary"] = self.boundary
        if self.env:
            payload["env"] = dict(self.env)
        return payload


ModuleSpecDecorator = Callable[[ModuleSpec], ModuleSpec]
ModuleToolHandler = Callable[[Any, dict[str, Any]], Awaitable[Any] | Any]
ModuleToolDecorator = Callable[[ModuleToolHandler], ModuleToolHandler]


@dataclass(frozen=True)
class ModuleTool:
    name: str
    handler: ModuleToolHandler
    description: str | None = None


@dataclass(frozen=True)
class ModuleDefinition:
    spec: ModuleSpec
    tools: tuple[ModuleTool, ...]
    description: str | None = None


def build_module_spec(
    *,
    module_id: str,
    command: str,
    args: Sequence[str] | None = None,
    restart_policy: RestartPolicy = "never",
    boundary: ModuleBoundary | None = None,
    env: Mapping[str, str] | None = None,
) -> ModuleSpec:
    return ModuleSpec(
        id=module_id,
        command=command,
        args=tuple(args or ()),
        restart_policy=restart_policy,
        boundary=boundary,
        env=dict(env or {}),
    )


def define_module_spec(
    *,
    module_id: str,
    command: str,
    args: list[str] | None = None,
    restart_policy: RestartPolicy = "never",
    boundary: ModuleBoundary | None = None,
    env: Mapping[str, str] | None = None,
) -> dict[str, object]:
    return build_module_spec(
        module_id=module_id,
        command=command,
        args=args,
        restart_policy=restart_policy,
        boundary=boundary,
        env=env,
    ).to_dict()


def decorate_module_spec(spec: ModuleSpec, *decorators: ModuleSpecDecorator) -> ModuleSpec:
    current = ModuleSpec(
        id=spec.id,
        command=spec.command,
        args=tuple(spec.args),
        restart_policy=spec.restart_policy,
        boundary=spec.boundary,
        env=dict(spec.env),
    )
    for decorate in decorators:
        current = decorate(current)
    return current


def decorate_module_tool(
    handler: ModuleToolHandler, *decorators: ModuleToolDecorator
) -> ModuleToolHandler:
    wrapped = handler
    for decorate in reversed(decorators):
        wrapped = decorate(wrapped)
    return wrapped


def define_module_tool(
    *,
    name: str,
    handler: ModuleToolHandler,
    description: str | None = None,
    decorators: Sequence[ModuleToolDecorator] | None = None,
) -> ModuleTool:
    wrapped = decorate_module_tool(handler, *(decorators or ()))
    return ModuleTool(name=name, handler=wrapped, description=description)


def define_module(
    *,
    spec: ModuleSpec,
    tools: Sequence[ModuleTool] | None = None,
    description: str | None = None,
) -> ModuleDefinition:
    return ModuleDefinition(
        spec=ModuleSpec(
            id=spec.id,
            command=spec.command,
            args=tuple(spec.args),
            restart_policy=spec.restart_policy,
            boundary=spec.boundary,
            env=dict(spec.env),
        ),
        tools=tuple(tools or ()),
        description=description,
    )
