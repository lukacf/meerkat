"""Public portable contract, authored before the extraction exists."""
from __future__ import annotations

import importlib
from copy import deepcopy
import pytest
from meerkat_toolkit.frontend.telegram import ProviderResponse


def api():
    return importlib.import_module('meerkat_toolkit.frontend.telegram_adapter')


def message(update=41):
    return {'update_id':update,'message':{'message_id':7,'date':1700000000,
        'chat':{'id':-99,'type':'supergroup','title':'fixture'},
        'from':{'id':3,'is_bot':False,'first_name':'Alex','last_name':'Example'},
        'message_thread_id':12,'reply_to_message':{'message_id':6},'text':'unchanged 🦊'}}


def test_portable_event_preserves_thread_and_caller_bytes():
    data=message();original=deepcopy(data)
    event=api().normalize_update(data)
    assert event.update_id==41
    assert event.envelope=={'provider':'telegram','provider_event_id':41,'provider_message_id':7,
        'sender_id':'3','sender_name':'Alex Example','recipient_or_chat_id':'-99','group_id':'-99',
        'thread_id':'12','reply_to_message_id':'6','text':'unchanged 🦊','timestamp':1700000000}
    assert event.media==()
    assert data==original
    data['message']['text']='mutated'
    assert event.envelope['text']=='unchanged 🦊'


@pytest.mark.parametrize('kind',['photo','document','voice'])
def test_portable_media_record(kind):
    data=message();data['message'].pop('text');data['message']['caption']='caption'
    if kind=='photo':
        data['message'][kind]=[{'file_id':'small','file_unique_id':'s','width':1,'height':1},
          {'file_id':'large','file_unique_id':'l','width':3,'height':2,'file_size':9}]
        expected=('large',9,'image/jpeg',None)
    elif kind=='document':
        data['message'][kind]={'file_id':'doc','file_unique_id':'d','file_name':'a.pdf',
                              'mime_type':'Application/PDF; charset=x','file_size':4}
        expected=('doc',4,'application/pdf','a.pdf')
    else:
        data['message'][kind]={'file_id':'voice','file_unique_id':'v','duration':2,'file_size':5}
        expected=('voice',5,'audio/ogg',None)
    event=api().normalize_update(data);ref=event.media[0]
    assert event.envelope['text']=='caption'
    assert (ref.provider_ref,ref.claimed_size_bytes,ref.claimed_mime_type,ref.filename)==expected
    assert ref.part_index==0


def test_page_cursor_is_only_returned_candidate_and_unsupported_is_distinct():
    m=api();page=m.interpret_poll_page((message(),{'update_id':44,'edited_message':{}}),'40')
    assert len(page.events)==1
    assert page.next_cursor=='45'
    assert page.ignored_updates==1
    assert m.poll_cursor_offset('00040')==40
    empty=m.interpret_poll_page((),'00040')
    assert empty.events==() and empty.next_cursor=='00040'


@pytest.mark.parametrize('bad',[{'update_id':True},{'update_id':42,'message':{}}])
def test_complete_page_refuses_later_invalid_record(bad):
    with pytest.raises(api().TelegramAdapterError):
        api().interpret_poll_page((message(),bad),'40')


class Transport:
    def __init__(self,responses=()):self.responses=list(responses);self.calls=[]
    async def send_message(self,**kw):
        self.calls.append(kw)
        result=self.responses.pop(0)
        if isinstance(result,Exception):raise result
        return result
    async def send_photo(self,**kw):return await self.send_message(**kw)
    async def get_file(self,ref):self.calls.append(('get',ref));return {'file_path':'p/file.png'}
    async def download_file(self,path,max_bytes):self.calls.append(('download',path,max_bytes));return b'\x00\xffraw'


@pytest.mark.asyncio
async def test_presenter_advises_plain_retry_without_a_second_provider_call():
    m=api();p=m.TelegramPresenter()
    t=Transport([ProviderResponse(400,{'ok':False,'error_code':400,'description':'bad entities'}),
                 ProviderResponse(200,{'ok':True,'result':{'message_id':9,'chat':{'id':-99}}})])
    receipt=await p.send_text(t,recipient='-99',text='hello',entities=({'type':'bold','offset':0,'length':5},))
    assert receipt.outcome=='known_not_sent' and receipt.provider_message_id is None
    assert receipt.retry_without_formatting is True
    assert len(t.calls)==1 and t.calls[0]['entities']
    assert len(t.responses)==1
    assert p.entity_wire_rejections==1 and p.entity_strip_fallbacks==0


@pytest.mark.asyncio
@pytest.mark.parametrize('response',[ProviderResponse(400,{'ok':False}),RuntimeError('private diagnostic')])
async def test_presenter_uncertain_outcome_never_replays(response):
    p=api().TelegramPresenter();t=Transport([response])
    receipt=await p.send_text(t,recipient='-99',text='hello',entities=({'type':'bold','offset':0,'length':5},))
    assert receipt.outcome=='unknown'
    assert receipt.retry_without_formatting is False
    assert len(t.calls)==1 and p.entity_wire_rejections==0


@pytest.mark.asyncio
async def test_caption_presentation_retains_bytes_and_rejects_links():
    p=api().TelegramPresenter();t=Transport([ProviderResponse(200,{'ok':True,'result':{'message_id':0,'chat':{'id':-99}}})])
    r=await p.send_photo(t,recipient='-99',data=b'\x00\xff',mime_type='image/png',caption='hi',
        caption_entities=({'type':'text_link','offset':0,'length':2,'url':'https://invalid.test'},))
    assert t.calls==[{'recipient':'-99','data':b'\x00\xff','mime_type':'image/png','caption':'hi','caption_entities':None}]
    assert r.outcome=='provider_queued' and p.entity_strip_fallbacks==1


@pytest.mark.asyncio
async def test_media_fetch_uses_borrowed_transport_and_exact_bound():
    m=api();t=Transport();r=await m.fetch_media(t,m.TelegramMediaRef(0,'opaque',filename='a.png'),16)
    assert t.calls==[('get','opaque'),('download','p/file.png',16)]
    assert r.data==b'\x00\xffraw' and r.mime_type=='image/png'


@pytest.mark.asyncio
@pytest.mark.parametrize('photo', [False, True])
@pytest.mark.parametrize('fallback', ['none', 'local', 'provider'])
async def test_explicit_topic_survives_all_formatting_paths(photo, fallback):
    presenter = api().TelegramPresenter()
    accepted = ProviderResponse(200, {'ok': True, 'result': {
        'message_id': 9, 'chat': {'id': -99}, 'message_thread_id': 12}})
    responses = [accepted]
    if fallback == 'provider':
        responses.insert(0, ProviderResponse(400, {
            'ok': False, 'error_code': 400, 'description': 'bad entities'}))
    transport = Transport(responses)
    entities = ({'type': 'bold', 'offset': 0, 'length': 5},)
    if fallback == 'local':
        entities = ({'type': 'text_link', 'offset': 0, 'length': 5,
                     'url': 'https://invalid.test'},)
    if photo:
        receipt = await presenter.send_photo(
            transport, recipient='-99', message_thread_id=12, data=b'\x00\xff',
            mime_type='image/png', caption='hello', caption_entities=entities)
    else:
        receipt = await presenter.send_text(
            transport, recipient='-99', message_thread_id=12, text='hello', entities=entities)
    assert receipt.outcome == ('known_not_sent' if fallback == 'provider' else 'provider_accepted')
    assert receipt.retry_without_formatting is (fallback == 'provider')
    assert len(transport.calls) == 1
    assert all(call['recipient'] == '-99' and call['message_thread_id'] == 12
               for call in transport.calls)
    if photo:
        assert all(call['data'] == b'\x00\xff' and call['caption'] == 'hello'
                   for call in transport.calls)
    else:
        assert all(call['text'] == 'hello' for call in transport.calls)
    field = 'caption_entities' if photo else 'entities'
    if fallback == 'provider':
        assert transport.calls[0][field] and len(transport.responses) == 1
    elif fallback == 'local':
        assert transport.calls[0][field] is None


@pytest.mark.asyncio
@pytest.mark.parametrize('photo', [False, True])
@pytest.mark.parametrize('entities', [(), ({'type': 'text_link', 'offset': 0, 'length': 5,
                                         'url': 'https://invalid.test'},)])
async def test_plain_wire_refusal_has_no_formatting_retry_advice(photo, entities):
    presenter = api().TelegramPresenter()
    transport = Transport([ProviderResponse(400, {
        'ok': False, 'error_code': 400, 'description': 'bad request'})])
    if photo:
        receipt = await presenter.send_photo(
            transport, recipient='-99', data=b'photo', mime_type='image/png',
            caption='hello', caption_entities=entities)
    else:
        receipt = await presenter.send_text(
            transport, recipient='-99', text='hello', entities=entities)
    assert receipt.outcome == 'known_not_sent'
    assert receipt.retry_without_formatting is False
    assert len(transport.calls) == 1
    assert presenter.entity_wire_rejections == 0


@pytest.mark.asyncio
@pytest.mark.parametrize('response', [
    ProviderResponse(429, {'ok': False, 'error_code': 429, 'description': 'rate limited',
                          'parameters': {'retry_after': 10}}),
    ProviderResponse(400, {'ok': False, 'error_code': 400, 'description': 'bad request',
                          'result': {}}),
    ProviderResponse(200, {'ok': True, 'result': {'message_id': 0, 'chat': {'id': -99}}}),
])
async def test_other_outcomes_never_advise_formatting_change(response):
    transport = Transport([response])
    receipt = await api().TelegramPresenter().send_text(
        transport, recipient='-99', text='hello',
        entities=({'type': 'bold', 'offset': 0, 'length': 5},))
    assert receipt.retry_without_formatting is False
    assert len(transport.calls) == 1


@pytest.mark.asyncio
@pytest.mark.parametrize('photo', [False, True])
async def test_invalid_topic_fails_before_borrowed_transport_entry(photo):
    presenter = api().TelegramPresenter()
    transport = Transport()
    for invalid in [True, False, 0, -1, 2147483648, '12', 12.0, {}, []]:
        with pytest.raises(ValueError, match='message_thread_id'):
            if photo:
                await presenter.send_photo(
                    transport, recipient='-99', message_thread_id=invalid,
                    data=b'photo', mime_type='image/png')
            else:
                await presenter.send_text(
                    transport, recipient='-99', message_thread_id=invalid, text='hello')
    assert transport.calls == []
    assert presenter.entity_strip_fallbacks == presenter.entity_wire_rejections == 0


@pytest.mark.asyncio
async def test_topic_is_per_call_and_absence_preserves_legacy_transport_keywords():
    accepted = ProviderResponse(200, {'ok': True, 'result': {'message_id': 9, 'chat': {'id': -99}}})
    presenter = api().TelegramPresenter()
    transport = Transport([
        ProviderResponse(200, {'ok': True, 'result': {
            'message_id': 9, 'chat': {'id': -99}, 'message_thread_id': topic}})
        for topic in [12, 13]
    ] + [accepted])
    for topic in [12, 13, None]:
        await presenter.send_text(
            transport, recipient='-99', message_thread_id=topic, text='hello')
    assert [call.get('message_thread_id') for call in transport.calls] == [12, 13, None]
    assert 'message_thread_id' not in transport.calls[-1]

    class LegacyTransport:
        async def send_message(self, *, recipient, text, entities=None):
            return accepted

        async def send_photo(self, *, recipient, data, mime_type, caption, caption_entities=None):
            return accepted

    legacy = LegacyTransport()
    assert (await presenter.send_text(legacy, recipient='-99', text='hello')).outcome == 'provider_accepted'
    assert (await presenter.send_photo(
        legacy, recipient='-99', data=b'photo', mime_type='image/png')).outcome == 'provider_accepted'
