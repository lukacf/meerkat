#!/usr/bin/env python3
"""Stage one host-selected GitOps proposal locally; never approve or deploy it."""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import re
import resource
import selectors
import signal
import stat
import subprocess
import sys
import time
import unicodedata

DOCUMENT_LIMIT = 2 * 1024 * 1024
MEMBER_LIMIT = 32 * 1024 * 1024
TREE_LIMIT = 128 * 1024 * 1024
TREE_ENTRIES = 10000
OBJECT_STORE_LIMIT = 256 * 1024 * 1024
PIN = re.compile(r'[0-9a-f]{64}\Z')
OBJECT = re.compile(r'[0-9a-f]{40}\Z')


class Refusal(ValueError):
    pass


def need(ok, code):
    if not ok:
        raise Refusal(code)


def sha(raw):
    return hashlib.sha256(raw).hexdigest()


def encoded(value):
    return json.dumps(value, sort_keys=True, ensure_ascii=True, allow_nan=False, indent=2).encode() + b'\n'


def pairs(rows):
    result = {}
    for key, value in rows:
        need(key not in result, 'duplicate_json_key')
        result[key] = value
    return result


def parse(raw):
    return json.loads(raw, object_pairs_hook=pairs,
                      parse_constant=lambda _: (_ for _ in ()).throw(Refusal('invalid_json')))


def read_file(path, limit=DOCUMENT_LIMIT):
    fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    try:
        info = os.fstat(fd)
        need(stat.S_ISREG(info.st_mode) and info.st_size <= limit, 'invalid_file')
        with os.fdopen(fd, 'rb', closefd=False) as stream:
            raw = stream.read(limit + 1)
        need(len(raw) <= limit, 'file_limit')
        return raw
    finally:
        os.close(fd)


def pinned(path, digest, limit=DOCUMENT_LIMIT):
    need(isinstance(digest, str) and PIN.fullmatch(digest), 'invalid_digest')
    raw = read_file(path, limit)
    need(sha(raw) == digest, 'input_changed')
    return raw


def relative(name):
    need(isinstance(name, str) and name and len(name.encode()) <= 1024, 'invalid_path')
    p = PurePosixPath(name)
    need(not p.is_absolute() and str(p) == name and '..' not in p.parts
         and '\\' not in name and not any(ord(c) < 32 or ord(c) == 127 for c in name)
         and all(unicodedata.normalize('NFC', part).casefold() != '.git' for part in p.parts), 'invalid_path')
    return p


def overlaps(a, b):
    left, right = [unicodedata.normalize('NFC', str(p)).casefold() for p in (a, b)]
    return left == right or left.startswith(right + '/') or right.startswith(left + '/')


def write_new(path, raw, mode=0o600):
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, mode)
    try:
        with os.fdopen(fd, 'wb', closefd=False) as stream:
            stream.write(raw)
            stream.flush()
            os.fsync(fd)
    finally:
        os.close(fd)


def child_limits():
    resource.setrlimit(resource.RLIMIT_FSIZE, (OBJECT_STORE_LIMIT, OBJECT_STORE_LIMIT))


class Runner:
    def __init__(self, git, deadline, cwd, git_sha256=None):
        self.git = str(git)
        self.deadline = deadline
        self.cwd = cwd
        self.calls = []
        self.git_sha256 = git_sha256
        self.env = {'PATH': '/usr/bin:/bin', 'HOME': str(cwd), 'LANG': 'C',
                    'GIT_CONFIG_NOSYSTEM': '1', 'GIT_CONFIG_SYSTEM': '/dev/null',
                    'GIT_CONFIG_GLOBAL': '/dev/null', 'GIT_ATTR_NOSYSTEM': '1',
                    'GIT_NO_REPLACE_OBJECTS': '1', 'GIT_NO_LAZY_FETCH': '1',
                    'GIT_TERMINAL_PROMPT': '0', 'GIT_OPTIONAL_LOCKS': '0',
                    'GIT_ALLOW_PROTOCOL': '', 'GIT_CEILING_DIRECTORIES': str(cwd)}

    def check(self):
        need(time.monotonic() < self.deadline, 'deadline_exceeded')

    def run(self, argv, data=b'', limit=DOCUMENT_LIMIT):
        self.check()
        child = subprocess.Popen([str(x) for x in argv], cwd=self.cwd, env=self.env,
                                 stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                 start_new_session=True, preexec_fn=child_limits)
        self.calls.append({'argv': [str(x) for x in argv], 'pid': child.pid})
        streams = selectors.DefaultSelector()
        outputs = {'stdout': bytearray(), 'stderr': bytearray()}
        pending = memoryview(data)
        try:
            for file, label in [(child.stdout, 'stdout'), (child.stderr, 'stderr')]:
                os.set_blocking(file.fileno(), False)
                streams.register(file, selectors.EVENT_READ, label)
            if pending:
                os.set_blocking(child.stdin.fileno(), False)
                streams.register(child.stdin, selectors.EVENT_WRITE, 'stdin')
            else:
                child.stdin.close()
            while streams.get_map():
                self.check()
                for key, _ in streams.select(min(0.1, max(0, self.deadline - time.monotonic()))):
                    if key.data == 'stdin':
                        try:
                            count = os.write(key.fd, pending[:65536])
                            pending = pending[count:]
                        except BrokenPipeError:
                            pending = memoryview(b'')
                        if not pending:
                            streams.unregister(key.fileobj)
                            key.fileobj.close()
                    else:
                        chunk = os.read(key.fd, 65536)
                        if not chunk:
                            streams.unregister(key.fileobj)
                            key.fileobj.close()
                        else:
                            outputs[key.data].extend(chunk)
                            need(len(outputs[key.data]) <= (limit if key.data == 'stdout' else 65536), 'child_output_limit')
            child.wait(timeout=max(0.001, self.deadline - time.monotonic()))
            self.calls[-1].update(exit_code=child.returncode, stdout_sha256=sha(outputs['stdout']),
                                  stderr_sha256=sha(outputs['stderr']))
            need(child.returncode == 0, 'child_failed')
            return bytes(outputs['stdout'])
        finally:
            # A second cancellation must not interrupt the bounded cleanup.
            handlers = {sig: signal.signal(sig, signal.SIG_IGN)
                        for sig in (signal.SIGTERM, signal.SIGHUP, signal.SIGINT)}
            try:
                streams.close()
                # Our children have fresh process groups. Reap helpers even on interruption.
                try:
                    os.killpg(child.pid, signal.SIGTERM)
                except ProcessLookupError:
                    pass
                try:
                    child.wait(timeout=2)
                except subprocess.TimeoutExpired:
                    pass
                try:
                    os.killpg(child.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
                child.wait(timeout=2)
            finally:
                self.calls[-1].update(exit_code=child.poll(), process_reaped=child.returncode is not None,
                                     stdout_sha256=sha(outputs['stdout']), stderr_sha256=sha(outputs['stderr']))
                for file in (child.stdin, child.stdout, child.stderr):
                    file.close()
                for sig, handler in handlers.items():
                    signal.signal(sig, handler)

    def command(self, *args, data=b'', limit=DOCUMENT_LIMIT):
        if self.git_sha256 is not None:
            pinned(Path(self.git), self.git_sha256, 128 * 1024 * 1024)
        result = self.run([self.git, '-c', 'core.hooksPath=/dev/null', '-c', 'core.fsmonitor=false',
                         '-c', 'core.attributesFile=/dev/null', '-c', 'gc.auto=0',
                         '-c', 'maintenance.auto=false', '-c', 'pack.threads=1', '-c', 'pack.packSizeLimit=0', *args], data, limit)
        if self.git_sha256 is not None:
            pinned(Path(self.git), self.git_sha256, 128 * 1024 * 1024)
        return result


def source_metadata(source, deadline):
    gitdir = source / '.git'
    need(gitdir.is_dir() and not gitdir.is_symlink(), 'ordinary_repository_required')
    for name in ['shallow', 'info/grafts', 'objects/info/alternates', 'objects/info/http-alternates']:
        need(not os.path.lexists(gitdir / name), 'unsupported_source_repository')
    objects = gitdir / 'objects'
    need(objects.is_dir() and not objects.is_symlink(), 'invalid_object_store')
    count = total = 0
    for directory, dirs, files in os.walk(objects, followlinks=False):
        need(time.monotonic() < deadline, 'deadline_exceeded')
        for name in dirs + files:
            path = Path(directory) / name
            info = path.lstat()
            need(not stat.S_ISLNK(info.st_mode) and (stat.S_ISDIR(info.st_mode) or stat.S_ISREG(info.st_mode)),
                 'invalid_object_store')
            need(not name.endswith('.promisor'), 'unsupported_source_repository')
            count += 1
            total += info.st_size if stat.S_ISREG(info.st_mode) else 0
            need(count <= 100000 and total <= OBJECT_STORE_LIMIT, 'object_store_limit')
    return gitdir, read_file(gitdir / 'config')


def tree(runner, commit):
    raw = runner.command('ls-tree', '-r', '-z', '--full-tree', commit, limit=8 * DOCUMENT_LIMIT)
    entries, paths = {}, {}
    for row in raw.split(b'\0'):
        if not row:
            continue
        meta, filename = row.split(b'\t', 1)
        mode, kind, oid = meta.decode('ascii').split(' ')
        name = filename.decode('utf-8')
        p = relative(name)
        need(mode in ['100644', '100755'] and kind == 'blob' and OBJECT.fullmatch(oid), 'unsupported_tree_entry')
        need(name not in entries and len(entries) < TREE_ENTRIES, 'tree_entry_limit')
        for i in range(1, len(p.parts) + 1):
            path = '/'.join(p.parts[:i])
            normalized = unicodedata.normalize('NFC', path).casefold()
            need(normalized not in paths or paths[normalized] == path, 'aliased_tree_path')
            paths[normalized] = path
        entries[name] = (mode, oid)
    return entries


def blobs(runner, entries):
    ids = sorted({value[1] for value in entries.values()})
    raw = runner.command('cat-file', '--batch', data=('\n'.join(ids) + '\n').encode(),
                         limit=TREE_LIMIT + TREE_ENTRIES * 128)
    result, offset, total = {}, 0, 0
    for oid in ids:
        end = raw.index(b'\n', offset)
        actual, kind, size = raw[offset:end].decode('ascii').split(' ')
        size = int(size)
        need(actual == oid and kind == 'blob' and 0 <= size <= MEMBER_LIMIT, 'invalid_blob')
        offset = end + 1
        value = raw[offset:offset + size]
        need(len(value) == size and raw[offset + size:offset + size + 1] == b'\n', 'invalid_blob')
        need(hashlib.sha1(b'blob ' + str(size).encode() + b'\0' + value).hexdigest() == oid, 'object_digest_mismatch')
        result[oid] = value
        total += size
        need(total <= TREE_LIMIT, 'tree_size_limit')
        offset += size + 1
    need(offset == len(raw), 'invalid_blob')
    return result


def stage(args):
    deadline = time.monotonic() + args.timeout_seconds
    need(0 < args.timeout_seconds <= 300, 'invalid_timeout')
    for value in [args.source_repository, args.request, args.planner, args.git, args.output]:
        need(value.is_absolute(), 'absolute_path_required')
    source = args.source_repository.resolve(strict=True)
    request_path, planner, git = [p.resolve(strict=True) for p in (args.request, args.planner, args.git)]
    parent = args.output.parent.resolve(strict=True)
    output = parent / args.output.name
    need(not any(ord(c) < 32 or ord(c) == 127 for p in (source, request_path, planner, git, output)
                 for c in str(p)), 'invalid_host_path')
    need(output.name not in ('', '.', '..') and not os.path.lexists(output), 'output_exists')
    need(not any(overlaps(output, p) for p in [source, request_path, planner, git]), 'overlapping_paths')
    request_raw = pinned(request_path, args.request_sha256)
    pinned(planner, args.planner_sha256, 128 * 1024 * 1024)
    pinned(git, args.git_sha256, 128 * 1024 * 1024)
    need(os.access(planner, os.X_OK) and os.access(git, os.X_OK) and PIN.fullmatch(args.proposal_sha256), 'invalid_tool_or_digest')
    request = parse(request_raw)
    profile_raw = request['profile_json'].encode()
    profile = parse(profile_raw)
    need(sha(profile_raw) == request['profile_sha256'], 'profile_digest_mismatch')
    commit, repository = profile['source_commit'], profile['source_repository']
    need(isinstance(commit, str) and OBJECT.fullmatch(commit), 'sha1_commit_required')
    gitdir, config = source_metadata(source, deadline)
    # This exclusive envelope is retained on failure. Presence alone is not success.
    output.mkdir(mode=0o700)
    runner = Runner(git, deadline, output, args.git_sha256)
    try:
        config_rows = runner.command('config', '--file', str(gitdir / 'config'), '--no-includes', '--list', '-z')
        for row in config_rows.split(b'\0'):
            key = row.split(b'\n', 1)[0].lower()
            need(not (key.startswith(b'extensions.') or key.endswith(b'.promisor')
                      or key == b'include.path' or key.startswith(b'includeif.')), 'unsupported_source_config')
        origin = runner.command('config', '--file', str(gitdir / 'config'), '--no-includes', '--get-all', 'remote.origin.url')
        need(origin == (repository + '\n').encode(), 'origin_mismatch')
        version = runner.command('--version').decode().strip()
        pinned(planner, args.planner_sha256, 128 * 1024 * 1024)
        raw = runner.run([planner], request_raw)
        pinned(planner, args.planner_sha256, 128 * 1024 * 1024)
        result = parse(raw)
        need(result['schema_version'] == 1 and result['status'] == 'prepared'
             and result['proposal_sha256'] == args.proposal_sha256, 'proposal_mismatch')
        proposal = result['proposal']
        need(proposal['source_commit'] == commit and proposal['source_repository'] == repository
             and proposal['profile_sha256'] == request['profile_sha256'], 'proposal_source_mismatch')
        base = request['base_documents']
        need(proposal['base_sha256'] == {name: sha(value.encode()) for name, value in base.items()}, 'base_binding_mismatch')
        deployment, template = profile['deployment']['path'], profile['config_map']['path']
        snapshot_directory = str(relative(profile['config_map']['snapshot_directory']))
        need(set(base) == {deployment, template}, 'base_binding_mismatch')
        files = proposal['files']
        need(len(files) == 2 and deployment in files and template not in files, 'proposal_file_scope')
        for name, item in files.items():
            path = relative(name)
            need(set(item) == {'action', 'sha256', 'contents'} and sha(item['contents'].encode()) == item['sha256'], 'replacement_digest_mismatch')
            need((name == deployment and item['action'] == 'replace') or
                 (str(path.parent) == snapshot_directory and item['action'] == 'create'), 'proposal_file_scope')
        checkout = output / 'checkout'
        empty = output / 'empty-template'; empty.mkdir()
        runner.command('init', '--quiet', '--template=' + str(empty), str(checkout))
        runner.cwd = checkout
        alternate = checkout / '.git/objects/info/alternates'
        write_new(alternate, (str(gitdir / 'objects') + '\n').encode())
        need(runner.command('rev-parse', '--verify', commit + '^{commit}').strip() == commit.encode(), 'commit_mismatch')
        original_tree = tree(runner, commit)
        original_blobs = blobs(runner, original_tree)
        need(sum(len(original_blobs[oid]) for _, oid in original_tree.values()) <= TREE_LIMIT, 'tree_size_limit')
        for name, value in base.items():
            need(name in original_tree and original_blobs[original_tree[name][1]] == value.encode(), 'committed_base_mismatch')
        for name, item in files.items():
            if item['action'] == 'create':
                need(name not in original_tree and not any(overlaps(Path(name), Path(p)) for p in original_tree), 'create_path_exists')
        runner.command('update-ref', '--no-deref', 'HEAD', commit)
        runner.command('repack', '-a', '-d', '--quiet')
        alternate.unlink()
        source_metadata(checkout, deadline)
        runner.command('fsck', '--full', '--strict', '--no-reflogs')
        runner.command('read-tree', commit)
        for name, (mode, oid) in original_tree.items():
            runner.check()
            path = checkout / name
            path.parent.mkdir(parents=True, exist_ok=True)
            write_new(path, original_blobs[oid], 0o755 if mode == '100755' else 0o644)
        for name, item in files.items():
            runner.check()
            path = checkout / name
            path.parent.mkdir(parents=True, exist_ok=True)
            if item['action'] == 'replace':
                need(read_file(path, MEMBER_LIMIT) == base[name].encode(), 'replacement_base_changed')
                path.unlink()
            write_new(path, item['contents'].encode(), 0o755 if name in original_tree and original_tree[name][0] == '100755' else 0o644)
            oid = runner.command('hash-object', '-w', '--no-filters', '--stdin', data=item['contents'].encode()).strip().decode()
            need(OBJECT.fullmatch(oid), 'invalid_blob')
            mode = original_tree[name][0] if name in original_tree else '100644'
            runner.command('update-index', '--add', '--cacheinfo', mode + ',' + oid + ',' + name)
        runner.command('config', '--local', 'remote.origin.url', repository)
        staged_tree = runner.command('write-tree').strip().decode()
        need(OBJECT.fullmatch(staged_tree), 'invalid_tree')
        observed_tree = tree(runner, staged_tree)
        observed_blobs = blobs(runner, observed_tree)
        need(set(observed_tree) == set(original_tree) | set(files), 'staged_scope_mismatch')
        for name, (mode, oid) in observed_tree.items():
            expected = files[name]['contents'].encode() if name in files else original_blobs[original_tree[name][1]]
            need(observed_blobs[oid] == expected and read_file(checkout / name, MEMBER_LIMIT) == expected, 'staged_bytes_mismatch')
            need(mode == (original_tree[name][0] if name in original_tree else '100644'), 'staged_mode_mismatch')
        patch = runner.command('diff', '--cached', '--binary', '--full-index', '--no-ext-diff', '--no-textconv', commit, '--', limit=TREE_LIMIT)
        runner.check()
        need(source_metadata(source, deadline)[1] == config, 'source_config_changed')
        pinned(request_path, args.request_sha256)
        pinned(planner, args.planner_sha256, 128 * 1024 * 1024)
        pinned(git, args.git_sha256, 128 * 1024 * 1024)
        write_new(output / 'proposal.json', raw)
        write_new(output / 'staged.patch', patch)
        write_new(output / 'commands.json', encoded(runner.calls))
        receipt = {'schema_version': 1, 'status': 'staged_proposal_only', 'source_repository': repository,
                   'source_commit': commit, 'origin_observation': 'local_configuration_association_not_upstream_authentication',
                   'request_sha256': args.request_sha256, 'proposal_sha256': args.proposal_sha256,
                   'planner_sha256': args.planner_sha256, 'git_sha256': args.git_sha256, 'git_version': version,
                   'staged_tree': staged_tree, 'patch_sha256': sha(patch), 'proposal_file_sha256': sha(raw),
                   'files': {name: item['sha256'] for name, item in files.items()},
                   'exclusions': ['approval', 'image_verification', 'deployment', 'readiness', 'native_continuation']}
        runner.check()
        write_new(output / 'receipt.pending', encoded(receipt))
        runner.check()
        os.link(output / 'receipt.pending', output / 'receipt.json', follow_symlinks=False)
        (output / 'receipt.pending').unlink()
        return receipt
    except BaseException:
        try:
            write_new(output / 'failure.json', encoded({'status': 'unconfirmed_partial_output', 'commands': runner.calls,
                      'instruction': 'Preserve this output; a new invocation requires a new destination.'}))
        except OSError:
            pass
        raise


def interrupt(_signum, _frame):
    raise Refusal('interrupted')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ['source-repository', 'request', 'planner', 'git', 'output']:
        parser.add_argument('--' + name, required=True, type=Path)
    for name in ['request-sha256', 'proposal-sha256', 'planner-sha256', 'git-sha256']:
        parser.add_argument('--' + name, required=True)
    parser.add_argument('--timeout-seconds', required=True, type=float)
    args = parser.parse_args()
    for sig in [signal.SIGTERM, signal.SIGHUP, signal.SIGINT]:
        signal.signal(sig, interrupt)
    try:
        result = stage(args)
        print(json.dumps(result, sort_keys=True))
        return 0
    except (Refusal, OSError, ValueError, KeyError, TypeError, IndexError, AttributeError,
            subprocess.SubprocessError, RecursionError):
        print('gitops-staging: unconfirmed; preserve any output directory; no approval or deployment claimed', file=sys.stderr)
        return 2


if __name__ == '__main__':
    sys.exit(main())
