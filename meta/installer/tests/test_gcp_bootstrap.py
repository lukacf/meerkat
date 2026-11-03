"""Offline selection and checked guest-binary installation counterexamples."""
import hashlib
import io
import os
from pathlib import Path
import stat
import subprocess
import tarfile
import zipfile

import pytest

from launchkit.gcp_bootstrap import BOOTSTRAP_SCRIPT, RUNTIME_SETUP, bootstrap_script


def definition(*commands):
    return {"classes": [{"tools": {"mcp": [{"connection_id": str(index)} for index in range(len(commands))]}}],
            "mcpConnections": [{"id": str(index), "transport": "stdio", "command": command}
                               for index, command in enumerate(commands)]}


def checked_definition(*commands):
    value = definition(*commands)
    for index, connection in enumerate(value['mcpConnections']):
        connection['id'] = 'connection_' + str(index)
        value['classes'][0]['tools']['mcp'][index]['connection_id'] = connection['id']
    return value


@pytest.fixture
def guest():
    namespace = {'__name__': 'guest_fixture'}
    exec(compile(RUNTIME_SETUP, 'checked guest bootstrap', 'exec'), namespace)
    namespace['ROOT_UID'] = os.getuid()
    return namespace


@pytest.mark.parametrize(('commands', 'expected'), [
    (('npx',), (True, False)), (('node', 'npm'), (True, False)),
    (('uvx',), (False, True)), (('uv', 'npx'), (True, True)),
])
def test_only_granted_supported_commands_select_binaries(commands, expected):
    script = bootstrap_script(checked_definition(*commands))
    assert len(script.encode()) <= 8192
    assert subprocess.run(['bash', '-n'], input=script, text=True, capture_output=True).returncode == 0
    namespace = {'__name__': 'guest_fixture'}
    observed = []
    # Execute the actual generated setup call against a host-free recorder.
    code = script.split("<<'MOBKIT_PREREQUISITES'\n", 1)[1].split('\nMOBKIT_PREREQUISITES', 1)[0]
    setup, call = code.rsplit('\ninstall(', 1)
    exec(compile(setup, 'checked guest bootstrap', 'exec'), namespace)
    namespace['install'] = lambda node, uv: observed.append((node, uv))
    exec('install(' + call, namespace)
    assert observed == [expected]


def test_http_ungranted_unknown_and_private_paths_keep_exact_base_script():
    value = checked_definition('custom-command', '${PRIVATE_COMMAND}')
    value['mcpConnections'] += [{'id': 'unused', 'transport': 'stdio', 'command': 'npx'},
                                {'id': 'remote', 'transport': 'http', 'command': 'uvx'}]
    value['classes'][0]['tools']['mcp'].append({'connection_id': 'remote'})
    assert bootstrap_script(value) == BOOTSTRAP_SCRIPT
    assert bootstrap_script({'classes': [], 'mcpConnections': []}) == BOOTSTRAP_SCRIPT


def node_archive(extra=None):
    output = io.BytesIO()
    with tarfile.open(fileobj=output, mode='w:xz') as archive:
        for name, content in {'bin/node': b'checked-node', 'lib/npm.js': b'checked-npm', 'lib/npx.js': b'checked-npx'}.items():
            item = tarfile.TarInfo('node-fixture/' + name)
            item.size, item.mode = len(content), 0o755
            archive.addfile(item, io.BytesIO(content))
        for name in ('npm', 'npx'):
            item = tarfile.TarInfo('node-fixture/bin/' + name)
            item.type, item.linkname = tarfile.SYMTYPE, '../lib/' + name + '.js'
            archive.addfile(item)
        if extra: archive.addfile(extra)
    return output.getvalue()


@pytest.mark.parametrize('unsafe', ['escape', 'hardlink', 'outside-link', 'link-parent', 'duplicate', 'oversize'])
def test_node_archive_rejects_unsafe_paths_before_destination_writes(guest, unsafe):
    item = tarfile.TarInfo('node-fixture/extra')
    if unsafe == 'escape': item.name = 'node-fixture/../outside'
    if unsafe == 'hardlink': item.type, item.linkname = tarfile.LNKTYPE, 'node-fixture/bin/node'
    if unsafe == 'outside-link': item.type, item.linkname = tarfile.SYMTYPE, '../../outside'
    if unsafe == 'link-parent': item.name = 'node-fixture/bin/npm/child'
    if unsafe == 'duplicate': item.name = 'node-fixture/bin/node'
    if unsafe == 'oversize': item.size = 256 * 1024 * 1024 + 1
    # Oversized metadata needs no giant file payload.
    with pytest.raises((ValueError, OSError)):
        guest['node_files'](node_archive(item), 'node-fixture')


def test_checked_tree_is_idempotent_and_foreign_commands_or_changed_files_are_preserved(guest, tmp_path, monkeypatch):
    opt, local = tmp_path / 'opt', tmp_path / 'local'
    opt.mkdir()
    local.mkdir()
    opt.chmod(0o755)
    local.chmod(0o755)
    files, links = guest['node_files'](node_archive(), 'node-fixture')
    install = lambda: guest['install_tree']('node-fixture', files, links, ('node', 'npm', 'npx'), opt, local / 'bin')
    target = opt / 'node-fixture'
    original_chmod = Path.chmod
    def interrupt_final_permissions(path, mode, *args, **kwargs):
        if path == target and mode == 0o755:
            raise OSError('Fixture interruption before directory permissions were finalized.')
        return original_chmod(path, mode, *args, **kwargs)
    with monkeypatch.context() as interrupted:
        interrupted.setattr(Path, 'chmod', interrupt_final_permissions)
        with pytest.raises(OSError, match='Fixture interruption'):
            install()
    assert stat.S_IMODE(target.stat().st_mode) == 0o700
    for command in ('node', 'npm', 'npx'):
        assert not (local / 'bin' / command).exists()
        assert not (local / 'bin' / command).is_symlink()
    retained_files = {name: ((target / name).stat().st_ino,
                            stat.S_IMODE((target / name).stat().st_mode),
                            (target / name).read_bytes()) for name in files}
    assert {name: (mode, content) for name, (_, mode, content) in retained_files.items()} == {
        name: (mode, content) for name, (content, mode) in files.items()}
    retained_links = {name: ((target / name).lstat().st_ino,
                            os.readlink(target / name)) for name in links}
    assert {name: link for name, (_, link) in retained_links.items()} == {
        name: link for name, (link, _) in links.items()}
    install()
    assert stat.S_IMODE(target.stat().st_mode) == 0o755
    assert {name: ((target / name).stat().st_ino,
                   stat.S_IMODE((target / name).stat().st_mode),
                   (target / name).read_bytes()) for name in files} == retained_files
    assert {name: ((target / name).lstat().st_ino,
                   os.readlink(target / name)) for name in links} == retained_links
    install()
    assert (local / 'bin/npm').read_bytes() == b'checked-npm'
    node = opt / 'node-fixture/bin/node'
    node.write_bytes(b'changed-node')
    with pytest.raises(ValueError, match='file changed'): install()
    assert node.read_bytes() == b'changed-node'
    node.write_bytes(b'checked-node')
    (local / 'bin/npm').unlink()
    (local / 'bin/npm').write_bytes(b'foreign-command')
    with pytest.raises(ValueError, match='another installation'): install()
    assert (local / 'bin/npm').read_bytes() == b'foreign-command'


def test_download_checksum_network_and_redirect_refusals_have_no_install_side_effect(guest, monkeypatch):
    class Response(io.BytesIO):
        def geturl(self): return 'https://nodejs.org/example'
    monkeypatch.setattr(guest['urllib'].request, 'urlopen', lambda *args, **kwargs: Response(b'changed'))
    with pytest.raises(ValueError, match='checksum'): guest['download']('https://nodejs.org/example', '0' * 64)
    monkeypatch.setattr(Response, 'geturl', lambda _: 'http://nodejs.org/example')
    with pytest.raises(ValueError, match='HTTPS host'): guest['download']('https://nodejs.org/example', hashlib.sha256(b'changed').hexdigest())
    def unavailable(*args, **kwargs): raise OSError('offline fixture')
    monkeypatch.setattr(guest['urllib'].request, 'urlopen', unavailable)
    with pytest.raises(OSError): guest['download']('https://nodejs.org/example', '0' * 64)


def test_uv_reads_only_expected_binaries_and_rejects_duplicate_or_linked_entries(guest):
    def wheel(duplicate=False, link=False):
        output = io.BytesIO()
        with zipfile.ZipFile(output, 'w') as archive:
            for name in ('uv', 'uvx'):
                item = zipfile.ZipInfo('uv-0.12.23.data/scripts/' + name)
                item.external_attr = (0o120777 if link else 0o100755) << 16
                archive.writestr(item, b'checked-' + name.encode())
            if duplicate: archive.writestr('uv-0.12.23.data/scripts/uv', b'foreign')
        return output.getvalue()
    assert guest['uv_files'](wheel())[0]['bin/uvx'] == (b'checked-uvx', 0o755)
    with pytest.raises(ValueError): guest['uv_files'](wheel(link=True))
    with pytest.warns(UserWarning): duplicate = wheel(duplicate=True)
    with pytest.raises(ValueError): guest['uv_files'](duplicate)
