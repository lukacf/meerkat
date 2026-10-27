"""Experimental synchronous bridge for explicitly selected Agno functions.

The host supplies a configured Toolkit and the effect classification of each
selected function. This module supplies no credentials, grants, agent runtime,
retry policy, lifecycle management or background cancellation.
"""

from __future__ import annotations

from collections.abc import Mapping
from copy import deepcopy
from dataclasses import dataclass
import inspect
import json
import math
from typing import Any, Callable, Literal, get_type_hints

from agno.tools import Toolkit
from agno.tools.function import Function
from agno.utils.schema import AGNO_INJECTED_PARAMS, FRAMEWORK_INJECTED_PARAMS
from jsonschema import Draft202012Validator
from referencing import Registry


Effect = Literal["read", "write"]
WriteOutcome = Literal["not_applicable", "not_started", "unknown"]
FailureKind = Literal[
    "unsupported_function", "invalid_selection", "unknown_function",
    "invalid_arguments", "entrypoint_error", "invalid_result", "result_too_large",
    "server_busy", "queue_timeout",
]


class AgnoAdapterError(Exception):
    """Bounded bridge metadata without upstream exception text or arguments.

An error after entering a write cannot establish whether its external effect
occurred. No error is an instruction to retry.
"""

    def __init__(
        self, kind: FailureKind, *, effect: Effect | None = None, entered: bool = False,
    ) -> None:
        self.kind = kind
        self.effect = effect
        self.entered = entered
        self.write_outcome: WriteOutcome = (
            "not_applicable" if effect == "read" else "unknown" if entered else "not_started"
        )
        super().__init__(f"Agno adapter {kind}")


class _JsonTooLarge(ValueError):
    pass


def _bounded_json(value: Any, limit: int) -> Any:
    """Copy a plain JSON value without silently coercing unsupported objects."""
    nodes = 0

    def visit(item: Any, depth: int) -> None:
        nonlocal nodes
        nodes += 1
        if depth > 64 or nodes > 100_000:
            raise _JsonTooLarge()
        if type(item) is str:
            if len(item) > limit:
                raise _JsonTooLarge()
        elif item is None or type(item) in (bool, int):
            pass
        elif type(item) is float:
            if not math.isfinite(item):
                raise ValueError()
        elif type(item) is list:
            for child in item:
                visit(child, depth + 1)
        elif type(item) is dict:
            for key, child in item.items():
                if type(key) is not str:
                    raise ValueError()
                visit(key, depth + 1)
                visit(child, depth + 1)
        else:
            raise ValueError()

    visit(value, 0)
    parts: list[str] = []
    size = 0
    encoder = json.JSONEncoder(ensure_ascii=False, allow_nan=False, separators=(",", ":"))
    for part in encoder.iterencode(value):
        size += len(part.encode("utf-8"))
        if size > limit:
            raise _JsonTooLarge()
        parts.append(part)
    return json.loads("".join(parts))


def _reject_external_references(value: Any) -> None:
    if isinstance(value, dict):
        for key, child in value.items():
            if key in ("$ref", "$dynamicRef", "$recursiveRef"):
                if not isinstance(child, str) or not child.startswith("#"):
                    raise ValueError()
            _reject_external_references(child)
    elif isinstance(value, list):
        for child in value:
            _reject_external_references(child)


def _check_function(function: Function) -> None:
    # These need Agno's invocation lifecycle. Silently skipping them would
    # change the function's declared behavior or bypass its approval contract.
    for field in (
        "cache_results", "pre_hook", "post_hook", "tool_hooks", "requires_confirmation",
        "requires_user_input", "user_input_fields", "user_input_schema", "external_execution",
        "external_execution_silent", "approval_type", "stop_after_tool_call", "show_result",
        "_agent", "_team", "_run_context", "_images", "_videos", "_audios", "_files",
    ):
        if getattr(function, field, None):
            raise ValueError()
    if function.add_instructions and function.instructions:
        raise ValueError()
    entrypoint = function.entrypoint
    if entrypoint is None or not callable(entrypoint):
        raise ValueError()
    raw = inspect.unwrap(entrypoint)
    if any(check(raw) or check(entrypoint) for check in (
        inspect.iscoroutinefunction, inspect.isgeneratorfunction, inspect.isasyncgenfunction,
    )):
        raise ValueError()
    signature = inspect.signature(entrypoint)
    # Agno can fall back to partial injection metadata when type resolution
    # fails. This narrower direct-call profile requires complete inspection.
    get_type_hints(entrypoint)
    reserved = {"self", "return", *FRAMEWORK_INJECTED_PARAMS, *AGNO_INJECTED_PARAMS}
    if reserved.intersection(signature.parameters):
        raise ValueError()
    for parameter in signature.parameters.values():
        if parameter.kind not in (
            inspect.Parameter.POSITIONAL_OR_KEYWORD, inspect.Parameter.KEYWORD_ONLY,
        ):
            raise ValueError()


@dataclass(frozen=True)
class _Binding:
    name: str
    description: str | None
    parameters: dict[str, Any]
    effect: Effect
    entrypoint: Callable[..., Any]
    signature: inspect.Signature
    validator: Draft202012Validator


class AgnoToolkitAdapter:
    """Select synchronous functions from one already configured host Toolkit.

    Construction inspects only the selected functions. It never calls toolkit
    connect/close, instantiates another toolkit or chooses an account. Hosts
    must keep borrowed toolkit/client state scoped to the selected account.
    """

    def __init__(
        self,
        toolkit: Toolkit,
        *,
        functions: Mapping[str, Effect],
        max_argument_bytes: int = 65_536,
        max_result_bytes: int = 1_048_576,
    ) -> None:
        error: AgnoAdapterError | None = None
        bindings: dict[str, _Binding] = {}
        try:
            if not isinstance(toolkit, Toolkit) or not functions:
                raise ValueError()
            if any(type(limit) is not int or limit <= 0 for limit in (
                max_argument_bytes, max_result_bytes,
            )):
                raise ValueError()
            if toolkit.cache_results or (toolkit.add_instructions and toolkit.instructions):
                raise ValueError()
            available = toolkit.get_functions()
            for name, effect in functions.items():
                if type(name) is not str or effect not in ("read", "write"):
                    raise ValueError()
                if name not in available:
                    error = AgnoAdapterError("invalid_selection")
                    break
                original = available[name]
                if not isinstance(original, Function) or original.name != name:
                    raise ValueError()
                _check_function(original)
                if effect == "read" and original.has_side_effects is True:
                    raise ValueError()
                function = original.model_copy(deep=True)
                function.process_entrypoint()
                _check_function(function)
                if not function.skip_entrypoint_processing and not getattr(
                    function.entrypoint, "_wrapped_for_validation", False,
                ):
                    # Agno logs and retains the raw callable if wrapping fails.
                    # That fallback would silently lose typed conversion here.
                    raise ValueError()
                if function._framework_params:
                    raise ValueError()
                assert function.entrypoint is not None
                signature = inspect.signature(function.entrypoint)
                parameters = _bounded_json(function.parameters, 1_048_576)
                if parameters.get("type") != "object":
                    raise ValueError()
                # Missing schema properties can mean a framework-typed parameter
                # was hidden. A direct invocation cannot supply such context.
                if set(parameters.get("properties", {})) != set(signature.parameters):
                    raise ValueError()
                if (
                    "$schema" in parameters
                    and parameters["$schema"] != Draft202012Validator.META_SCHEMA["$id"]
                ):
                    raise ValueError()
                _reject_external_references(parameters)
                Draft202012Validator.check_schema(parameters)
                validator = Draft202012Validator(parameters, registry=Registry())
                bindings[name] = _Binding(
                    name, function.description, parameters, effect,
                    function.entrypoint, signature, validator,
                )
        except Exception:
            error = AgnoAdapterError("unsupported_function")
        if error is not None:
            raise error
        self._bindings = bindings
        self._max_argument_bytes = max_argument_bytes
        self._max_result_bytes = max_result_bytes

    def describe(self) -> list[dict[str, Any]]:
        """Return detached upstream schemas plus trusted host effect labels."""
        return [
            {
                "name": binding.name,
                "description": binding.description,
                "parameters": deepcopy(binding.parameters),
                "effect": binding.effect,
            }
            for binding in self._bindings.values()
        ]

    def invoke(self, name: str, arguments: dict[str, Any]) -> Any:
        """Validate, call once, and return a bounded detached JSON value.

        No result text is interpreted as a success receipt. In particular, an
        upstream string containing an error remains that exact string. This
        synchronous call has no deadline or cancellation machinery; host-level
        interruption after entry cannot establish that a write did not occur.
        """
        binding = self._bindings.get(name) if type(name) is str else None
        if binding is None:
            raise AgnoAdapterError("unknown_function")
        invalid = False
        try:
            if type(arguments) is not dict:
                raise ValueError()
            supplied = _bounded_json(arguments, self._max_argument_bytes)
            binding.validator.validate(supplied)
            binding.signature.bind(**supplied)
        except Exception:
            invalid = True
        if invalid:
            raise AgnoAdapterError("invalid_arguments", effect=binding.effect)

        error: AgnoAdapterError | None = None
        try:
            result = binding.entrypoint(**supplied)
        except Exception:
            error = AgnoAdapterError("entrypoint_error", effect=binding.effect, entered=True)
        if error is not None:
            raise error

        try:
            if inspect.iscoroutine(result):
                result.close()
            elif inspect.isgenerator(result):
                result.close()
            result = _bounded_json(result, self._max_result_bytes)
        except _JsonTooLarge:
            error = AgnoAdapterError("result_too_large", effect=binding.effect, entered=True)
        except Exception:
            error = AgnoAdapterError("invalid_result", effect=binding.effect, entered=True)
        if error is not None:
            raise error
        return result
