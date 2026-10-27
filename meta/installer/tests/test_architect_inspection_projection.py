"""Blueprint inspection stays independent of optional native model reads."""
import asyncio
import copy
from dataclasses import asdict
from types import SimpleNamespace
from unittest.mock import AsyncMock, Mock

import pytest

from launchkit.architect import ArchitectService
from launchkit.blueprint import Blueprint, starter_blueprint
from launchkit.bootstrap import CORE_VERSION, VERSION
from test_architect import native_catalog


@pytest.fixture
def architect(tmp_path):
    service = ArchitectService(tmp_path)
    service.set_context(starter_blueprint().model_dump(mode='json'))
    return service


@pytest.mark.asyncio
@pytest.mark.parametrize('args', [{}, {'include_schema': True}], ids=['default', 'schema-only'])
async def test_default_inspection_does_not_enter_a_blocked_native_catalogue(architect, args):
    entered, release = asyncio.Event(), asyncio.Event()

    async def blocked_catalogue():
        entered.set()
        await release.wait()
        return native_catalog()

    catalogue = AsyncMock(side_effect=blocked_catalogue)
    architect.runtime = SimpleNamespace(mob_handle=lambda: SimpleNamespace(models_catalog=catalogue))
    before = copy.deepcopy(architect.context)
    reading = asyncio.create_task(architect.inspect(args))
    catalogue_entered = asyncio.create_task(entered.wait())
    try:
        await asyncio.wait({reading, catalogue_entered}, timeout=1, return_when=asyncio.FIRST_COMPLETED)
        assert reading.done(), 'Ordinary design inspection must not wait for an optional model catalogue.'
        result = await reading
        assert result['blueprint'] == before
        assert result['model_choices'] is None
        assert result['schema'] == (Blueprint.model_json_schema() if args.get('include_schema') else None)
        assert result['design_reference_loaded'] is False
        assert 'topic:all before proposing' in result['design_reference']
        catalogue.assert_not_awaited()
        assert architect.model_catalog_snapshot is None
    finally:
        release.set()
        catalogue_entered.cancel()
        await asyncio.gather(reading, catalogue_entered, return_exceptions=True)


@pytest.mark.asyncio
async def test_default_inspection_does_not_query_or_cache_an_unavailable_catalogue(architect):
    catalogue = AsyncMock(side_effect=RuntimeError('private-catalogue-diagnostic'))
    architect.runtime = SimpleNamespace(mob_handle=lambda: SimpleNamespace(models_catalog=catalogue))
    result = await architect.inspect({})
    assert result['model_choices'] is None
    catalogue.assert_not_awaited()
    assert architect.model_catalog_runtime is None and architect.model_catalog_snapshot is None
    assert 'private-catalogue-diagnostic' not in str(result)


@pytest.mark.asyncio
async def test_explicit_model_choices_preserve_the_complete_detached_native_result(architect):
    catalog = native_catalog()
    native = asdict(catalog)
    catalogue = AsyncMock(return_value=catalog)
    architect.runtime = SimpleNamespace(mob_handle=lambda: SimpleNamespace(models_catalog=catalogue))
    observed = await architect.inspect({'include_model_choices': True})
    choices = observed['model_choices']
    assert choices['models'] == native['models']
    assert choices['provider_defaults'] == native['provider_defaults']
    assert choices['status'] == 'observed'
    assert choices['source'] == 'native_runtime_curated_catalog'
    assert choices['method'] == 'runtime.mob_handle().models_catalog()'
    assert choices['native_version'] == VERSION and choices['core_version'] == CORE_VERSION
    exact = copy.deepcopy(choices)
    assert exact == await architect.model_choices()
    choices['models'][0]['id'] = 'caller-edited-model'
    choices['provider_defaults'][0]['models'][0]['id'] = 'caller-edited-default'
    assert (await architect.inspect({'include_model_choices': True}))['model_choices'] == exact
    assert asdict(catalog) == native
    for topic in ('all', 'capabilities'):
        reference = await architect.read_design_reference({'topic': topic})
        assert reference['model_choices'] == exact
        reference['model_choices']['models'][0]['id'] = 'reference-caller-edit'
    assert await architect.model_choices() == exact
    catalogue.assert_awaited_once_with()


@pytest.mark.asyncio
@pytest.mark.parametrize('flag', [1, 'true', None], ids=['integer', 'string', 'null'])
async def test_invalid_model_choices_flag_is_rejected_before_saved_or_native_reads(architect, flag):
    architect.guide_context = {'guide_id': 'current-guide', 'action': 'chat', 'brief': {}}
    saved_review = Mock(return_value={'retained': 'private-review'})
    architect.saved_toolkit_review_provider = saved_review
    catalogue = AsyncMock(return_value=native_catalog())
    architect.runtime = SimpleNamespace(mob_handle=lambda: SimpleNamespace(models_catalog=catalogue))
    before = copy.deepcopy((architect.context, architect.guide_context))
    with pytest.raises(ValueError, match='include_model_choices'):
        await architect.inspect({'include_model_choices': flag})
    saved_review.assert_not_called()
    catalogue.assert_not_awaited()
    assert (architect.context, architect.guide_context) == before
