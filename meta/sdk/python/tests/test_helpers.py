from meerkat_meta.helpers import build_module_spec, define_module, define_module_spec


def test_module_spec_boundary_serializes_for_gateway_init():
    spec = build_module_spec(
        module_id="router",
        command="python3",
        args=["router.py"],
        boundary="mcp",
    )

    assert spec.to_dict() == {
        "id": "router",
        "command": "python3",
        "args": ["router.py"],
        "restart_policy": "never",
        "boundary": "mcp",
    }


def test_define_module_spec_keeps_boundary_and_env_out_when_empty():
    assert define_module_spec(module_id="delivery", command="python3") == {
        "id": "delivery",
        "command": "python3",
        "args": [],
        "restart_policy": "never",
    }


def test_define_module_preserves_module_boundary_and_env_copy():
    spec = build_module_spec(
        module_id="router",
        command="python3",
        boundary="mcp",
        env={"ROUTER_FIXTURE": "example"},
    )

    definition = define_module(spec=spec)
    definition.spec.env["ROUTER_FIXTURE"] = "other"

    assert definition.spec.boundary == "mcp"
    assert spec.env == {"ROUTER_FIXTURE": "example"}
