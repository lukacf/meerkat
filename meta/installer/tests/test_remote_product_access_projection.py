"""Actual retained product access projection with inert external boundaries.

The original source Install, factory, actor IPC and public access owner remain
real. The reused fixture's SDK/native health, forward and HTTP checks are inert.
These controls establish no runtime qualification, SSH, cloud or useful task.
"""
from copy import deepcopy

from launchkit.remote_browser_access import access_binding
from test_remote_product_access import (protected_package, protected_owner,
    original_control_package, original_control_destination, original_controls,
    control_destination, inputs_destination, destination, installation,
    destination_source, artifact_custody, product, no_destination_execution,
    no_product_execution)
from test_remote_browser_access import access
from test_remote_deployment import service


async def test_original_product_public_console_and_custom_projection_retains_existing_instance_bindings(protected_owner):
    c = protected_owner
    expected = access_binding(c.observed)
    assert c.observed['worker_live'] is True and c.observed['service'] == {}
    for entry_id in ('native-console', 'application:workspace'):
        value = await c.browser._closed_public(c.observed, entry_id)
        assert value['status'] == 'closed' and value['local_url'] is None
        assert value['native_ready'] is False and value['cleanup_confirmed'] is True
        assert value['service_name'] is value['service_source_digest'] is None
        assert value['console_url'] == expected['console_url']
        for key in ('product_control_binding', 'product_runtime_binding'):
            assert value[key] == expected[key]
            assert value[key] is not expected[key]
        value['product_control_binding']['worker']['start_ticks'] = 'foreign-result-mutation'
        value['product_runtime_binding']['runtime_pid'] = -1
        assert access_binding(c.observed) == expected
    ready = await c.browser.open(c.gid, c.state['version'], c.observed['build_id'], c.did)
    assert ready['status'] == 'ready' and ready['native_ready'] is True
    assert ready['scope']['authentication'] == 'local-password'
    assert ready['product_control_binding'] == expected['product_control_binding']
    assert ready['product_runtime_binding'] == expected['product_runtime_binding']
    key = (c.gid, c.did)
    original = deepcopy(c.browser.records[key]['binding'])
    ready['product_runtime_binding']['startup_attempt_id'] = 'foreign-result-mutation'
    assert c.browser.records[key]['binding'] == original
    assert c.observed['worker_live'] is True and c.observed['service'] == {}


async def test_ordinary_closed_public_keeps_original_legacy_service_shape_without_product_fields(access):
    browser, receipt = access
    value = await browser._closed_public(receipt)
    assert value['service_name'] == receipt['service']['service']
    assert value['service_source_digest'] == receipt['service']['source_digest']
    assert value['console_url'] == receipt['service']['console_url']
    assert 'product_control_binding' not in value and 'product_runtime_binding' not in value
    assert value['status'] == 'closed' and value['local_url'] is None
