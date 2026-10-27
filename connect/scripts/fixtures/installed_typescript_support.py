#!/usr/bin/env python3
"""Artifact-only support for the installed TypeScript owner.

Reuse the reviewed native preparation finalizer. This process never constructs
a Meta runtime, returns admission, or supplies a native grant reference.
"""
import asyncio
import dataclasses
import hashlib
import importlib
import importlib.util
import json
import os
from pathlib import Path
import sys


def canonical(value):
    return json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode()


def load_verifier(pin, sdk_pin):
    path = Path(pin["path"])
    if path.is_symlink() or hashlib.sha256(path.read_bytes()).hexdigest() != pin["sha256"]:
        raise AssertionError("selected verifier changed")
    spec = importlib.util.spec_from_file_location("installed_ts_verifier", path)
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    module.SDK = importlib.import_module("meerkat_toolkit.host_control")
    actual = Path(module.SDK.__file__).resolve()
    if actual != Path(sdk_pin["path"]).resolve() or hashlib.sha256(actual.read_bytes()).hexdigest() != sdk_pin["sha256"]:
        raise AssertionError("selected Python host module changed")
    return module


async def main():
    command, input_path, result_path = sys.argv[1:]
    selected = json.loads(Path(input_path).read_bytes())
    v = load_verifier(selected["verifier"], selected["python_host_control"])
    directory = Path(selected["output"])
    events = v.EventLog(directory)
    finalization = directory / "finalization"
    report = {"command": command, "status": "failed", "scope": "artifact commands only; no runtime owner"}
    finalizer = None
    try:
        if command == "prepare":
            environment = v.process_env(directory)
            empty_path = directory / "empty-path"
            empty_path.mkdir(mode=0o700)
            environment.update(PATH=str(empty_path), META_GATEWAY_STDERR_FILE=str(directory / "gateway.stderr"))
            roster = selected["roster"]
            v.require([row["identity"] for row in roster] == ["helper", "original"], "fixture roster differs")
            plan, plan_pin, source_pin = v.installed_plan(directory, roster)
            finalization.mkdir(mode=0o700)
            finalizer = v.InstalledFixtureFinalizer(Path(selected["gateway"]["path"]), finalization,
                plan, plan_pin, source_pin, environment, events, lambda: None)
            inventory_pin = v.new_artifact(directory / "inventory-input.json", canonical({
                "schema": 1, "mob_id": "fixture", "roster": roster, "helper_identity": "helper"}))
            inventory = await finalizer.command("inventory", ["--lower-application-inventory", "--input", inventory_pin["path"]])
            v.require(inventory["schema"] == 1 and inventory["mob_id"] == "fixture"
                and inventory["realm_id"] == "mob.fixture" and inventory["helper_identity"] == "helper"
                and [row["identity"] for row in inventory["members"]] == ["helper", "original"],
                "native inventory differs from the selected roster")
            members = {row["identity"]: row["member"] for row in inventory["members"]}
            v.require(members["helper"] == plan["application"]["helper"]["member"]
                and members["original"]["mob_id"] == "fixture" and members["original"]["role"] == "original"
                and members["original"]["member"] != members["helper"]["member"], "native member binding differs")
            principal = v.installed_fixture_principal
            source_owner, requester, executor = principal("ts-source"), principal("ts-requester"), principal("ts-original")
            target = {"member": members["original"], "logical_owner": executor, "context_generation": 1,
                "audience": {"audience": "principal", "principal": requester}}
            native = plan["owner"]["native_host"]
            native["members"].append(target)
            plan["application"]["members"] = native["members"]
            plan["owner"]["components"] = ["ts-source"]
            plan["owner"]["channels"] = [{"component": "ts-source", "channel_id": "ts-loopback",
                "service_subject": "ts-source", "provider_account": "ts-fixture", "sender_id": "ts-requester",
                "requester": requester, "ingress_actor": source_owner, "target_identity": "original",
                "member": members["original"], "realm": "mob.fixture", "required_surfaces": ["channel:ts-loopback"]}]
            native["routes"] = [{"id": "ts-original", "origin": {"kind": "provider_event", "channel_id": "ts-loopback",
                "sender_id": "ts-requester", "target_identity": "original"}, "application_parent": None,
                "controller_parent": None, "logical_executor": executor, "represented_subject": None,
                "source_authority": source_owner, "authentication_domain": {"authority": source_owner, "namespace": "ts-auth"},
                "event_domain": {"authority": source_owner, "namespace": "ts-events"}}]
            plan["authorities"]["controllers"].append({"target": {"kind": "original_route", "route": "ts-original"},
                "issuance_id": "ts-original-controller-parent"})
            plan["application"]["definition"]["text"] += '''
[profiles.original]
external_addressable = true
model = "fixture-model"
provider = "self_hosted"
self_hosted_server_id = "local"
runtime_mode = "turn_driven"
[profiles.original.tools]
comms = true
'''
            source = json.loads(Path(source_pin["path"]).read_bytes())
            source.update(purpose="installed TypeScript owner, one controlled input and no acquisition",
                definition=plan["application"]["definition"], source_route=native["routes"][0],
                source_channel=plan["owner"]["channels"][0], target=target)
            source_pin = v.new_artifact(directory / "ts-policy.json", canonical(source))
            policy = plan["application"]["tool_policy"]
            policy["source"]["source_digest"] = v.installed_fixture_policy_digest(canonical(source))
            policy["members"].append({"member_identity": members["original"]["member"], "grants": []})
            plan_pin = v.new_artifact(directory / "ts-plan.json", canonical(plan))
            config_pin = v.new_artifact(directory / "model.toml", v.config_text(selected["model_port"]).encode())
            report.update(status="passed", plan=plan_pin, source=source_pin, model_config=config_pin,
                environment=environment, inventory=inventory)
        elif command == "finalize":
            prepared = json.loads((directory / "prepare-result.json").read_bytes())
            v.require(prepared["status"] == "passed", "artifact preparation did not finish")
            plan = json.loads(v.exact_bytes(Path(prepared["plan"]["path"]), prepared["plan"]["sha256"]))
            finalizer = v.InstalledFixtureFinalizer(Path(selected["gateway"]["path"]), finalization,
                plan, prepared["plan"], prepared["source"], prepared["environment"], events, lambda: None)
            manifest = v.SDK.SelectedHostArtifact(**selected["manifest"])
            selection = await finalizer(manifest)
            report.update(status="passed", selection=dataclasses.asdict(selection))
        else:
            raise AssertionError("unsupported artifact command")
    except BaseException as error:
        report["error_type"] = type(error).__name__
        raise
    finally:
        if finalizer is not None:
            report["finalization"] = finalizer.evidence
        report["events"] = events.rows
        report["python_host_control"] = selected["python_host_control"]
        Path(result_path).write_bytes(canonical(report) + b"\n")


if __name__ == "__main__":
    asyncio.run(main())
