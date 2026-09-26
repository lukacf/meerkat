#!/usr/bin/env python3
"""Mutation-sensitive tests for generated RPC transport coupling."""

from __future__ import annotations

import importlib.util
import json
import re
import sys
import tempfile
import unittest
from pathlib import Path

SCRIPT = Path(__file__).with_name("verify_rpc_signature_parity.py")
SPEC = importlib.util.spec_from_file_location("verify_rpc_signature_parity", SCRIPT)
assert SPEC and SPEC.loader
parity = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = parity
SPEC.loader.exec_module(parity)

GENERATOR = SCRIPT.parent.parent / "tools/sdk-codegen/generate.py"
GENERATOR_SPEC = importlib.util.spec_from_file_location("sdk_codegen", GENERATOR)
assert GENERATOR_SPEC and GENERATOR_SPEC.loader
sdk_codegen = importlib.util.module_from_spec(GENERATOR_SPEC)
sys.modules[GENERATOR_SPEC.name] = sdk_codegen
GENERATOR_SPEC.loader.exec_module(sdk_codegen)

CATALOG = {"demo/get": {"params": "DemoParams", "result": "DemoResult"}}
GENERATED = {"DemoParams", "DemoResult"}


class GeneratedTransportTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)
        (self.root / "sdks/typescript/src/generated").mkdir(parents=True)
        (self.root / "sdks/python/meerkat/generated").mkdir(parents=True)
        (self.root / "artifacts/schemas").mkdir(parents=True)

    def tearDown(self) -> None:
        self.temp.cleanup()

    def write_ts(self, contracts: str, client: str) -> None:
        (self.root / "sdks/typescript/src/generated/rpc_contracts.ts").write_text(
            contracts
        )
        (self.root / "sdks/typescript/src/client.ts").write_text(client)

    def write_py(self, contracts: str, client: str) -> None:
        (self.root / "sdks/python/meerkat/generated/rpc_contracts.py").write_text(
            contracts
        )
        (self.root / "sdks/python/meerkat/client.py").write_text(client)

    def test_typescript_requires_the_actual_generic_transport(self) -> None:
        self.write_ts(
            '"demo/get": { params: DemoParams; result: '
            "(DemoResult) & Record<string, unknown>; };",
            "type _RpcSignature = [DemoParams, DemoResult];\n",
        )
        failures = parity.check_generated_transport_contracts(
            self.root, CATALOG, sdk="ts", generated=GENERATED
        )
        self.assertTrue(any("not generic" in failure for failure in failures))
        self.assertTrue(any("marker" in failure for failure in failures))

    def test_typescript_missing_method_entry_fails(self) -> None:
        self.write_ts(
            "",
            "request<M extends RpcMethodName>(method: M, "
            'params: RpcMethodContracts[M]["params"]): '
            'Promise<RpcMethodContracts[M]["result"]> { throw 0; }',
        )
        failures = parity.check_generated_transport_contracts(
            self.root, CATALOG, sdk="ts", generated=GENERATED
        )
        self.assertIn("ts: generated RPC contract missing `demo/get`", failures)

    def test_python_requires_binding_to_generated_overloads(self) -> None:
        self.write_py(
            'method: Literal["demo/get"], params: DemoParams, /, '
            ") -> Awaitable[DemoResult]: ...",
            "async def _request_impl(self): pass\n",
        )
        failures = parity.check_generated_transport_contracts(
            self.root, CATALOG, sdk="py", generated=GENERATED
        )
        self.assertTrue(any("not bound" in failure for failure in failures))

    def test_catalog_bound_transports_pass(self) -> None:
        self.write_ts(
            '"demo/get": { params: DemoParams; result: '
            "(DemoResult) & Record<string, unknown>; };",
            "request<M extends RpcMethodName>(method: M, "
            'params: RpcMethodContracts[M]["params"]): '
            'Promise<RpcMethodContracts[M]["result"]> { throw 0; }',
        )
        self.write_py(
            'method: Literal["demo/get"], params: DemoParams, /, '
            ") -> Awaitable[DemoResult]: ...",
            "_request: RpcRequest\n"
            "self._request = cast(RpcRequest, self._request_impl)\n"
            "async def _request_impl(self): pass\n",
        )
        self.assertEqual(
            parity.check_generated_transport_contracts(
                self.root, CATALOG, sdk="ts", generated=GENERATED
            ),
            [],
        )
        self.assertEqual(
            parity.check_generated_transport_contracts(
                self.root, CATALOG, sdk="py", generated=GENERATED
            ),
            [],
        )

    def test_python_required_params_are_checked_at_the_send_site(self) -> None:
        (self.root / "artifacts/schemas/wire-types.json").write_text(
            '{"DemoParams":{"type":"object","properties":'
            '{"session_id":{"type":"string"}},'
            '"required":["session_id"]}}'
        )
        self.write_py(
            'method: Literal["demo/get"], params: DemoParams, /, '
            ") -> Awaitable[DemoResult]: ...",
            "class Demo:\n"
            "    async def send(self):\n"
            '        return await self._request("demo/get", '
            '{"sessoin_id": "s1"})\n',
        )
        failures = parity.check_python_required_param_shapes(self.root, CATALOG)
        self.assertTrue(any("params shape drift" in failure for failure in failures))

        self.write_py(
            'method: Literal["demo/get"], params: DemoParams, /, '
            ") -> Awaitable[DemoResult]: ...",
            "class Demo:\n"
            "    async def send(self):\n"
            '        payload: DemoParams = {"sessoin_id": "s1"}\n'
            '        return await self._request("demo/get", payload)\n',
        )
        failures = parity.check_python_required_param_shapes(self.root, CATALOG)
        self.assertTrue(any("params shape drift" in failure for failure in failures))

        self.write_py(
            'method: Literal["demo/get"], params: DemoParams, /, '
            ") -> Awaitable[DemoResult]: ...",
            "class Demo:\n"
            "    async def send(self):\n"
            '        return await self._request("demo/get", '
            '{"session_id": "s1"})\n',
        )
        self.assertEqual(
            parity.check_python_required_param_shapes(self.root, CATALOG), []
        )

    def test_outer_object_fields_are_merged_into_one_of_variants(self) -> None:
        schema = {
            "type": "object",
            "properties": {"session_id": {"type": "string"}},
            "required": ["session_id"],
            "oneOf": [
                {
                    "type": "object",
                    "properties": {"kind": {"const": "generic_json"}},
                    "required": ["kind"],
                },
                {
                    "type": "object",
                    "properties": {"kind": {"const": "peer_response_terminal"}},
                    "required": ["kind"],
                },
            ],
        }
        typed = sdk_codegen._one_of_typed_dict_variants(schema, schema)
        self.assertIsNotNone(typed)
        assert typed is not None
        _, variants = typed
        self.assertEqual(len(variants), 2)
        for _, variant in variants:
            self.assertIn("session_id", variant["properties"])
            self.assertIn("session_id", variant["required"])


class GeneratedEventReferenceTests(unittest.TestCase):
    def setUp(self) -> None:
        self.root = {
            "$defs": {
                "Payload": {
                    "type": "object",
                    "properties": {
                        "session_id": {"type": "string"},
                        "detail": {"type": "string"},
                    },
                    "required": ["session_id"],
                }
            }
        }
        self.variant = {
            "$ref": "#/$defs/Payload",
            "type": "object",
            "properties": {"type": {"const": "payload_ready"}},
            "required": ["type"],
        }

    def test_plain_event_reference_keeps_the_named_type(self) -> None:
        for ref in (
            {"$ref": "#/$defs/Payload"},
            {"$ref": "#/$defs/Payload", "description": "Named payload"},
        ):
            with self.subTest(ref=ref):
                self.assertEqual(
                    sdk_codegen._web_events_ts_type(self.root, ref), "Payload"
                )

    def test_tagged_event_reference_keeps_payload_and_discriminator(self) -> None:
        rendered = sdk_codegen._web_events_ts_type(self.root, self.variant)
        self.assertEqual(
            rendered,
            '{\n  session_id: string;\n  detail?: string;\n'
            '  type: "payload_ready";\n}',
        )
        self.assertNotIn("type", self.root["$defs"]["Payload"]["properties"])
        self.assertEqual(self.variant["required"], ["type"])

    def test_web_event_interface_merges_tagged_reference_requirements(self) -> None:
        schemas = {
            "events": {"AgentEvent": {**self.root, "oneOf": [self.variant]}}
        }
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory)
            sdk_codegen.generate_web_event_types(schemas, output)
            rendered = (output / "events.ts").read_text()
        self.assertIn(
            'export interface PayloadReadyEvent {\n  session_id: string;\n'
            '  detail?: string;\n  type: "payload_ready";\n}',
            rendered,
        )
        self.assertIn("export type AgentEvent =\n  PayloadReadyEvent;", rendered)

    def test_canonical_discard_event_keeps_public_payload_in_both_sdks(self) -> None:
        schema_path = GENERATOR.parents[2] / "artifacts/schemas/events.json"
        schemas = {"events": json.loads(schema_path.read_text())}
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory)
            sdk_codegen.generate_web_event_types(schemas, output)
            web = (output / "events.ts").read_text()
            # Supply the prior emitter's declarations without touching SDK files.
            (output / "types.ts").write_text("")
            sdk_codegen.generate_typescript_event_types(schemas, output)
            typescript = (output / "event_types.ts").read_text()
        web_event = re.search(
            r"export interface BoundaryAppendsDiscardedEvent \{([^}]+)\}", web
        )
        self.assertIsNotNone(web_event)
        assert web_event is not None
        expected_fields = {
            "input_ids: InputId[];",
            "run_id: RunId;",
            "session_id: SessionId;",
            'type: "boundary_appends_discarded";',
        }
        self.assertEqual(
            {line.strip() for line in web_event[1].splitlines() if line.strip()},
            expected_fields,
        )
        agent_event = re.search(
            r"export type AgentEvent = (.*?)(?=\nexport |\Z)",
            typescript,
            re.DOTALL,
        )
        self.assertIsNotNone(agent_event)
        assert agent_event is not None
        tagged_arm = re.search(
            r'\{([^{}]*type: "boundary_appends_discarded";[^{}]*)\}',
            agent_event[1],
        )
        self.assertIsNotNone(tagged_arm)
        assert tagged_arm is not None
        self.assertEqual(
            {line.strip() for line in tagged_arm[1].splitlines() if line.strip()},
            expected_fields,
        )


if __name__ == "__main__":
    unittest.main()
