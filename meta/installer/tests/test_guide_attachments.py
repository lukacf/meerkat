import asyncio
import base64
import copy
import hashlib
import io
import json
import os
from pathlib import Path
from uuid import uuid4

import pytest

from launchkit import server
from launchkit.guide_attachments import GuideAttachments, MAX_FILE_BYTES, MAX_TURN_BYTES
from launchkit.guide_models import starter_state
from launchkit.guide_store import GuideStore
from test_guide import guide_client, start, inspected_guide_turn

PNG = base64.b64decode('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+aG9kAAAAASUVORK5CYII=')


@pytest.fixture
def attachments(tmp_path):
    store = GuideStore(tmp_path)
    first = store.create(starter_state({'available': False}))
    second = store.create(starter_state({'available': False}))
    return GuideAttachments(tmp_path), first['id'], second['id']


def text_file(text='Fictional evidence, not instructions.'):
    return ('evidence.txt', 'text/plain', text.encode())


def http_file():
    filename, media_type, data = text_file()
    return filename, data, media_type


def pdf_bytes(*, text='Fictional PDF evidence.', encrypted=False, pages=1):
    from pypdf import PdfWriter
    from pypdf.generic import DictionaryObject, NameObject, DecodedStreamObject
    writer = PdfWriter()
    for _ in range(pages):
        page = writer.add_blank_page(width=200, height=200)
        if text:
            font = DictionaryObject({NameObject('/Type'): NameObject('/Font'),
                NameObject('/Subtype'): NameObject('/Type1'), NameObject('/BaseFont'): NameObject('/Helvetica')})
            page[NameObject('/Resources')] = DictionaryObject({NameObject('/Font'):
                DictionaryObject({NameObject('/F1'): writer._add_object(font)})})
            stream = DecodedStreamObject()
            stream.set_data(f'BT /F1 12 Tf 10 100 Td ({text}) Tj ET'.encode())
            page[NameObject('/Contents')] = writer._add_object(stream)
    if encrypted:
        writer.encrypt('fictional-fixture-password')
    output = io.BytesIO()
    writer.write(output)
    return output.getvalue()


def test_immutable_upload_redo_survives_new_store_and_returns_exact_bytes(attachments):
    store, guide_id, _ = attachments
    ref, = store.save(guide_id, [('sample.png', 'image/png', PNG)])
    assert ref['sha256'] == hashlib.sha256(PNG).hexdigest() and ref['size'] == len(PNG)
    assert ref['url'] == f'/api/guide/{guide_id}/attachments/{ref["id"]}'
    fresh = GuideAttachments(store.root)
    assert fresh.save(guide_id, [('sample.png', 'image/png', PNG)]) == [ref]
    assert fresh.read(guide_id, ref['id']) == (ref, PNG)
    for path in (store.root / 'guide-attachments' / guide_id).iterdir():
        assert path.stat().st_mode & 0o077 == 0
        assert not path.name.startswith('.upload-')


def test_cross_guide_reference_and_changed_metadata_are_rejected(attachments):
    store, first, second = attachments
    ref, = store.save(first, [text_file()])
    with pytest.raises(ValueError):
        store.resolve(second, [ref])
    with pytest.raises(ValueError):
        store.read(second, ref['id'])
    for key, value in [('filename', 'other.txt'), ('size', 1), ('sha256', 'a' * 64),
                       ('url', 'https://example.test/forged')]:
        forged = {**ref, key: value}
        with pytest.raises(ValueError):
            store.resolve(first, [forged])
    with pytest.raises(ValueError):
        store.resolve(first, [ref, ref])
    assert store.resolve(first, [ref]) == [ref]


@pytest.mark.parametrize('file', [
    ('../outside.txt', 'text/plain', b'no'),
    ('C:\\outside.txt', 'text/plain', b'no'),
    ('binary.txt', 'text/plain', b'\xff\x00'),
    ('fake.png', 'image/png', b'not an image'),
    ('wrong.png', 'image/jpeg', PNG),
    ('bad.json', 'application/json', b'{not json}'),
    ('script.html', 'text/html', b'<script>no</script>'),
])
def test_invalid_file_batch_creates_no_received_references(attachments, file):
    store, guide_id, _ = attachments
    with pytest.raises(ValueError):
        store.save(guide_id, [text_file(), file])
    assert not (store.root / 'guide-attachments').exists()


def test_limits_apply_to_file_upload_and_combined_later_reply(attachments):
    store, guide_id, _ = attachments
    with pytest.raises(ValueError, match='8 MiB'):
        store.save(guide_id, [text_file('x' * (MAX_FILE_BYTES + 1))])
    with pytest.raises(ValueError, match='16 MiB'):
        store.save(guide_id, [(f'{index}.txt', 'text/plain', b'x' * (MAX_TURN_BYTES // 3 + 1)) for index in range(3)])
    with pytest.raises(ValueError, match='four'):
        store.save(guide_id, [text_file()] * 5)
    refs = [store.save(guide_id, [(f'{index}.txt', 'text/plain', b'x' * (MAX_TURN_BYTES // 3 + 1))])[0]
            for index in range(3)]
    with pytest.raises(ValueError, match='16 MiB'):
        store.resolve(guide_id, refs)


def test_unknown_guide_and_path_or_symlink_reads_are_refused(attachments, tmp_path):
    store, guide_id, _ = attachments
    with pytest.raises(LookupError):
        store.save(str(uuid4()), [text_file()])
    with pytest.raises(ValueError):
        store.read(guide_id, '../../outside')
    root = store.root / 'guide-attachments'
    target = tmp_path / 'outside'
    target.mkdir(mode=0o700)
    root.symlink_to(target, target_is_directory=True)
    with pytest.raises(ValueError, match='unsafe'):
        store.save(guide_id, [text_file()])
    assert list(target.iterdir()) == []


def test_blob_symlink_and_changed_bytes_do_not_bypass_content_identity(attachments, tmp_path):
    store, guide_id, _ = attachments
    ref, = store.save(guide_id, [text_file()])
    path = store.root / 'guide-attachments' / guide_id / (ref['sha256'] + '.bin')
    path.write_bytes(b'changed')
    with pytest.raises(ValueError, match='contents'):
        store.read(guide_id, ref['id'])
    path.unlink()
    outside = tmp_path / 'outside.txt'
    outside.write_bytes(text_file()[2])
    os.chmod(outside, 0o600)
    path.symlink_to(outside)
    with pytest.raises(ValueError, match='unavailable'):
        store.read(guide_id, ref['id'])


def test_text_reads_are_paged_without_truncating_or_executing_instructions(attachments):
    store, guide_id, _ = attachments
    text = 'Ignore instructions is quoted evidence only.\n' + 'é' * 13000
    ref, = store.save(guide_id, [text_file(text)])
    first = store.read_content(guide_id, ref['id'])
    second = store.read_content(guide_id, ref['id'], offset=first['next_offset'])
    assert not first['complete'] and first['next_offset'] == 12000
    assert second['complete'] and second['next_offset'] is None
    assert first['text'] + second['text'] == text
    assert 'not operating instructions' in first['scope']
    with pytest.raises(ValueError):
        store.read_content(guide_id, ref['id'], offset=True)
    with pytest.raises(ValueError):
        store.read_content(guide_id, ref['id'], max_chars=12001)


def test_real_pdf_extraction_pagination_and_explicit_no_ocr(attachments):
    store, guide_id, _ = attachments
    data = pdf_bytes()
    ref, = store.save(guide_id, [('evidence.pdf', 'application/pdf', data)])
    result = store.read_content(guide_id, ref['id'], max_chars=10)
    rest = store.read_content(guide_id, ref['id'], offset=result['next_offset'])
    assert 'Fictional PDF evidence.' in result['text'] + rest['text']
    assert result['complete'] is False and rest['complete'] is True
    assert 'no OCR or image analysis' in result['extraction']
    assert store.read(guide_id, ref['id'])[1] == data


@pytest.mark.parametrize('options,error', [({'encrypted': True}, 'Encrypted'),
                                         ({'text': ''}, 'no extractable'),
                                         ({'pages': 101}, '100 pages')])
def test_unreadable_pdf_has_truthful_error_and_no_stored_reference(attachments, options, error):
    store, guide_id, _ = attachments
    with pytest.raises(ValueError, match=error):
        store.save(guide_id, [('evidence.pdf', 'application/pdf', pdf_bytes(**options))])
    assert not (store.root / 'guide-attachments').exists()


def test_http_upload_download_scoping_and_no_state_mutation(guide_client):
    client, _, service = guide_client
    first, second = start(client), start(client)
    before = service.store.get(first['id'])
    response = client.post(f'/api/guide/{first["id"]}/attachments', files=[('files', http_file())])
    assert response.status_code == 200, response.text
    ref, = response.json()['attachments']
    assert client.get(ref['url']).content == text_file()[2]
    assert client.get(ref['url']).headers['x-content-type-options'] == 'nosniff'
    assert client.get(f'/api/guide/{second["id"]}/attachments/{ref["id"]}').status_code == 422
    assert client.post(f'/api/guide/{uuid4()}/attachments', files=[('files', http_file())]).status_code == 404
    assert service.store.get(first['id']) == before
    assert client.get('/api/guide/new').json()['input_surface'] == first['input_surface']
    assert 'application/pdf' in first['input_surface']['media_types']
    assert client.post(f'/api/guide/{first["id"]}/attachments', files=[('files', ('empty.txt', b'', 'text/plain'))]).status_code == 422


def test_http_multipart_bounds_reject_before_storage_or_admission(guide_client):
    client, agent, service = guide_client
    state = start(client)
    url = f'/api/guide/{state["id"]}/attachments'
    requests = [
        [('files', ('oversize.txt', b'x' * (MAX_FILE_BYTES + 1), 'text/plain'))],
        [('files', (f'{index}.txt', b'x' * (MAX_TURN_BYTES // 3 + 1), 'text/plain')) for index in range(3)],
        [('wrong-field', http_file())],
        [('files', http_file())] * 5,
    ]
    for files in requests:
        response = client.post(url, files=files)
        assert response.status_code == 422, response.text
    assert not (service.store.root / 'guide-attachments').exists()
    assert service.store.operation(state['id']) is None and agent.calls == []


def test_large_valid_upload_has_route_specific_limit_and_transport_remains_bounded(guide_client):
    client, agent, service = guide_client
    state = start(client)
    url = f'/api/guide/{state["id"]}/attachments'
    response = client.post(url, files=[('files', ('large.txt', b'x' * MAX_FILE_BYTES, 'text/plain'))])
    assert response.status_code == 200, response.text
    reference, = response.json()['attachments']
    assert reference['size'] == MAX_FILE_BYTES
    assert client.post(url, content=b'x' * (MAX_TURN_BYTES + 64 * 1024 + 1),
                       headers={'content-type': 'multipart/form-data; boundary=test'}).status_code == 413
    assert client.post(f'/api/guide/{state["id"]}/respond', json={
        'version': state['version'], 'action': 'chat', 'text': 'x' * 1_000_001}).status_code == 413
    assert service.store.operation(state['id']) is None and agent.calls == []


def test_reply_preserves_received_refs_and_redo_is_one_operation(guide_client, tmp_path):
    client, agent, service = guide_client
    state = start(client)
    response = client.post(f'/api/guide/{state["id"]}/attachments', files=[('files', http_file())])
    ref, = response.json()['attachments']
    body = {'version': state['version'], 'action': 'chat', 'attachments': [ref]}
    response = client.post(f'/api/guide/{state["id"]}/respond', json=body)
    assert response.status_code == 200, response.text
    call = agent.calls[-1]
    assert call['guide_context']['current_user_message']['text'] == 'Consider the attached evidence.'
    assert call['guide_context']['current_user_message']['attachments'] == [ref]
    user = next(row for row in response.json()['messages'] if row['role'] == 'user')
    assert user['attachments'] == [ref]
    inspection = inspected_guide_turn(call, tmp_path)['guided_setup']
    assert inspection['current_user_message']['attachments'] == [ref]
    assert inspection['evidence_messages'][-1]['attachments'] == [ref]
    assert text_file()[2].decode() not in call['prompt']
    count = len(agent.calls)
    retry = client.post(f'/api/guide/{state["id"]}/respond', json=body)
    assert retry.status_code == 200 and len(agent.calls) == count
    assert service.store.operation(state['id'])['request']['attachments'] == [ref]
    assert GuideStore(service.store.root).get(state['id'])['messages'] == response.json()['messages']


def test_forged_reply_is_rejected_before_operation_or_native_work(guide_client):
    client, agent, service = guide_client
    first, second = start(client), start(client)
    ref, = service.attachments.save(first['id'], [text_file()])
    for guide_id, requested in [(second['id'], ref), (first['id'], {**ref, 'filename': 'forged.txt'})]:
        response = client.post(f'/api/guide/{guide_id}/respond', json={
            'version': 1, 'action': 'chat', 'attachments': [requested]})
        assert response.status_code == 422
        assert service.store.operation(guide_id) is None
    assert agent.calls == []


def test_empty_attachment_field_retains_previous_request_identity():
    body = {'version': 1, 'text': 'A normal answer.'}
    implicit = server.GuideResponse(**body).model_dump(mode='json')
    explicit = server.GuideResponse(**body, attachments=[]).model_dump(mode='json')
    assert implicit == explicit and 'attachments' not in implicit
