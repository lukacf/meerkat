"""A pending or rejected SDK install stays visibly in its actual stage.

This uses the original install worker and receipt owner with inert byte fixtures.
No provider, selected SDK, native gateway or application executes.
"""
import asyncio

import pytest

from launchkit import deployment, deployment_worker as worker
from test_meta_toolkit_initial_install import (worker_fixture, admitted, initial, no_processes,
    implementation_service)
from test_meta_toolkit_runtime_selection import product, no_product_execution


@pytest.mark.parametrize('reject', [False, True])
async def test_inflight_install_reports_stage_without_premature_completion(worker_fixture, monkeypatch, reject):
    f = worker_fixture
    entered, release = asyncio.Event(), asyncio.Event()
    original = worker.Supervisor.install_selected_sdk_pair
    async def held_pair(self, *args, **kwargs):
        entered.set()
        await release.wait()
        if reject:
            raise ValueError('Inert SDK install refusal.')
        return await original(self, *args, **kwargs)
    monkeypatch.setattr(worker.Supervisor, 'install_selected_sdk_pair', held_pair)
    task = asyncio.create_task(worker.run(f.directory, 'install'))
    try:
        await asyncio.wait_for(entered.wait(), 30)
        pending = deployment.read_receipt(f.directory)
        assert pending['status'] == 'installing' and pending['phase'] == 'install'
        assert pending['detail'] == 'Installing the agent software and its dependencies.'
        assert not pending.get('console_url') and not pending.get('runtime_pid')
        assert not pending.get('installed_candidate_runtime_proof')
        assert not (f.destination / '.bin').exists()
        assert not task.done()
        release.set()
        result = await asyncio.wait_for(task, 30)
        completed = deployment.read_receipt(f.directory)
        history = [event['message'] for event in completed['progress']]
        assert 'Preparing the application environment.' in history
        if reject:
            assert result == 1 and completed['status'] == 'failed'
            assert 'Copying and checking the native runtime files.' not in history
            assert 'Checking the installed runtime and application.' not in history
            assert not completed.get('installed_candidate_runtime_proof')
        else:
            assert result == 0 and completed['status'] == 'stopped' and completed['phase'] == 'configure'
            assert history.index('Installing the agent software and its dependencies.') < history.index(
                'Copying and checking the native runtime files.') < history.index(
                'Checking the installed runtime and application.')
            assert not completed.get('console_url') and not completed.get('runtime_pid')
    finally:
        release.set()
        await asyncio.gather(task, return_exceptions=True)
