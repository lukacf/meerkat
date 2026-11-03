#!/usr/bin/env python3
"""Compare explicitly pinned local library evidence; never install or approve it."""

from __future__ import annotations

import argparse
import base64
import csv
import gzip
import hashlib
import io
import json
import math
from pathlib import Path, PurePosixPath
import re
import stat
import sys
import tarfile
import zipfile
import zlib
from email.parser import BytesParser
from decimal import Decimal, InvalidOperation


MAX_FILE = 64 * 1024 * 1024
MAX_MEMBER = 32 * 1024 * 1024
MAX_TOTAL = 128 * 1024 * 1024
MAX_MEMBERS = 10000
TAR_METADATA_ALLOWANCE = 16 * 1024 * 1024
PIN = re.compile(r"[0-9a-f]{64}\Z")
NAME = re.compile(r"[A-Za-z0-9_.-]+\Z")


class CheckError(ValueError):
    """Value-free diagnostic: input content is never interpolated into errors."""


def require(condition, reason):
    if not condition:
        raise CheckError(reason)


def sha(data):
    return hashlib.sha256(data).hexdigest()


def encoded(value):
    return json.dumps(value, sort_keys=True, ensure_ascii=True, allow_nan=False, separators=(",", ":")).encode()


def object_pairs(pairs):
    result = {}
    for key, value in pairs:
        require(key not in result, "duplicate-json-key")
        result[key] = value
    return result


def exact_float(token):
    """Refuse numeric values changed by the finite float's decimal round-trip."""
    number = float(token)
    try:
        exact = Decimal(token)
    except InvalidOperation:
        raise CheckError("unsupported-lossy-json-number") from None
    require(math.isfinite(number) and Decimal(str(number)) == exact, "unsupported-lossy-json-number")
    return number


def parse(data):
    try:
        return json.loads(data, object_pairs_hook=object_pairs, parse_float=exact_float,
                          parse_constant=lambda _: (_ for _ in ()).throw(CheckError("nonfinite-json-number")))
    except (ValueError, UnicodeError, RecursionError) as exc:
        if isinstance(exc, CheckError):
            raise
        raise CheckError("invalid-json") from None


def record(value):
    require(isinstance(value, dict), "expected-record")
    return value


def exact(value, keys):
    require(set(record(value)) == set(keys), "unexpected-record-fields")


def relative(value):
    require(isinstance(value, str) and value and len(value) <= 1024, "invalid-relative-path")
    p = PurePosixPath(value)
    require(value != '.' and not p.is_absolute() and str(p) == value and ".." not in p.parts
            and "\\" not in value and not any(ord(c) < 32 for c in value), "invalid-relative-path")
    return p


def pin(value):
    exact(value, ["path", "sha256"])
    relative(value["path"])
    require(isinstance(value["sha256"], str) and PIN.fullmatch(value["sha256"]), "invalid-sha256")
    return value


def read_pin(root, reference):
    pin(reference)
    path = (root / reference["path"]).resolve(strict=True)
    require(path.is_relative_to(root.resolve()), "input-escapes-root")
    require(path.is_file() and path.stat().st_size <= MAX_FILE, "invalid-input-file-or-size")
    data = path.read_bytes()
    require(len(data) <= MAX_FILE and sha(data) == reference["sha256"], "input-digest-mismatch")
    return data


def bounded_tar(data):
    """Bound all expanded tar bytes, including metadata parsed before members."""
    limit = MAX_TOTAL + TAR_METADATA_ALLOWANCE
    with gzip.GzipFile(fileobj=io.BytesIO(data), mode='rb') as compressed:
        expanded = compressed.read(limit + 1)
    require(len(expanded) <= limit, 'expanded-tar-size-limit')
    return tarfile.open(fileobj=io.BytesIO(expanded), mode='r:')


def archive_payload(data, family):
    """Read bounded regular members in memory. Never extract or import code."""
    files, total = {}, 0

    def keep(name, size, read):
        nonlocal total
        relative(name)
        require(name not in files and len(files) < MAX_MEMBERS, "duplicate-or-excess-archive-member")
        require(0 <= size <= MAX_MEMBER and total + size <= MAX_TOTAL, "archive-size-limit")
        content = read(MAX_MEMBER + 1)
        require(len(content) == size, "archive-member-size-mismatch")
        files[name] = content
        total += size

    if family in ["agno-wheel", "services-wheel"]:
        with zipfile.ZipFile(io.BytesIO(data)) as archive:
            for member in archive.infolist():
                require(not member.is_dir() and not stat.S_ISLNK(member.external_attr >> 16)
                        and not member.flag_bits & 1, "unsupported-wheel-member")
                with archive.open(member) as stream:
                    keep(member.filename, member.file_size, stream.read)
    else:
        with bounded_tar(data) as archive:
            for member in archive:
                require(member.isfile() and member.name.startswith("package/"), "unsupported-npm-member")
                with archive.extractfile(member) as stream:
                    keep(member.name[len("package/"):], member.size, stream.read)
    require(files, "empty-archive")
    return files


def checked_member(files, name, digest):
    relative(name)
    require(isinstance(digest, str) and PIN.fullmatch(digest), "invalid-member-digest")
    require(name in files and sha(files[name]) == digest, "missing-or-mismatched-artifact-member")


def wheel_metadata(files, distribution):
    metadata = [p for p in files if p.endswith('.dist-info/METADATA')]
    require(len(metadata) == 1, "ambiguous-wheel-metadata")
    prefix = metadata[0].rsplit('/', 1)[0]
    headers = BytesParser().parsebytes(files[metadata[0]])
    require(len(headers.get_all('Name', [])) == len(headers.get_all('Version', [])) == 1,
            "ambiguous-wheel-identity")
    require(headers['Name'] == distribution and headers['Version'], "unexpected-wheel-identity")
    record_path = prefix + '/RECORD'
    require(record_path in files, "missing-wheel-record")
    seen = set()
    for row in csv.reader(io.StringIO(files[record_path].decode('utf-8'))):
        require(len(row) == 3 and row[0] not in seen and row[0] in files, "invalid-wheel-record")
        seen.add(row[0])
        if row[0] == record_path:
            require(row[1:] == ['', ''], "invalid-wheel-self-record")
        else:
            digest = base64.urlsafe_b64encode(hashlib.sha256(files[row[0]]).digest()).rstrip(b'=').decode()
            require(row[1] == 'sha256=' + digest and row[2] == str(len(files[row[0]])), "wheel-record-mismatch")
    require(seen == set(files), "incomplete-wheel-record")
    return headers, prefix


def wheel_facts(files):
    headers, prefix = wheel_metadata(files, 'meerkat-toolkit-agno')
    source = parse(files['meerkat_toolkit_agno/PACKAGE-SOURCES.json'])
    require(type(source['schema_version']) is int and source['schema_version'] == 1
            and source['distribution'] == headers['Name'], "invalid-wheel-source-manifest")
    sources = record(source['source_mapping'])
    for location, item in sources.items():
        require(location.startswith('src/'), "invalid-wheel-source-location")
        checked_member(files, location[len('src/'):], item['sha256'])
    require({p[len('src/'):] for p in sources} == {p for p in files if p.startswith('meerkat_toolkit_agno/') and p.endswith('.py')},
            "incomplete-wheel-source-manifest")
    for name in ['LICENSE-APACHE', 'LICENSE-MIT', 'LICENSE-Agno', 'THIRD_PARTY_NOTICES.md']:
        checked_member(files, prefix + '/licenses/' + name, source['packaging_sha256'][name])
    checked_member(files, 'meerkat_toolkit_agno/PROFILE-GUIDE.md', source['profile_guide']['sha256'])
    return {
        'identity': {'name': headers['Name'], 'version': headers['Version']},
        'source_files': sources,
        'build_dependencies': {},
        'configuration_docs': {'PROFILE-GUIDE.md': source['profile_guide']['sha256']},
        'package_requirements': {'requires_python': headers.get('Requires-Python'),
                                 'requires_dist': sorted(headers.get_all('Requires-Dist', []))},
    }


def services_facts(files):
    """Check shipped services bytes; keep absent source metadata as claims."""
    headers, prefix = wheel_metadata(files, 'meerkat-toolkit-services')
    namespace = 'meerkat_toolkit_services/'
    source = parse(files[namespace + 'PACKAGE-SOURCES.json'])
    exact(source, ['distribution', 'namespace', 'source_mapping', 'guide_mapping',
                   'other_payload_sha256', 'scope'])
    require(source['distribution'] == headers['Name'] and source['namespace'] == namespace[:-1]
            and isinstance(source['scope'], str), 'invalid-services-source-manifest')
    require(all(p.startswith(namespace) or p.startswith(prefix + '/') for p in files),
            'unsupported-services-wheel-layout')
    sources, guides = record(source['source_mapping']), record(source['guide_mapping'])
    expected, claims, docs = {namespace + 'PACKAGE-SOURCES.json', namespace + 'GUIDE.md'}, {}, {}
    require(sources, 'incomplete-wheel-source-manifest')
    for group, suffix in [(sources, '.py'), (guides, '.md')]:
        for location, item in group.items():
            relative(location)
            require(location.startswith('src/' + namespace) and location.endswith(suffix),
                    'invalid-wheel-source-location')
            member = location[len('src/'):]
            exact(item, ['canonical_source', 'sha256'] if suffix == '.py' else
                  ['canonical_source', 'canonical_sha256', 'sha256', 'literal_replacements'])
            relative(item['canonical_source'])
            checked_member(files, member, item['sha256'])
            expected.add(member)
            if suffix == '.md':
                pin({'path': item['canonical_source'], 'sha256': item['canonical_sha256']})
                require(isinstance(item['literal_replacements'], list), 'invalid-guide-projection')
                for replacement in item['literal_replacements']:
                    exact(replacement, ['from', 'to'])
                    require(all(isinstance(v, str) for v in replacement.values()), 'invalid-guide-projection')
                docs[member] = item['sha256']
                claims[location] = {key: item[key] for key in ['canonical_source', 'canonical_sha256', 'literal_replacements']}
    require({p[len('src/'):] for p in sources} == {p for p in files if p.endswith('.py')},
            'incomplete-wheel-source-manifest')
    require(expected == {p for p in files if p.startswith(namespace)}, 'incomplete-services-payload-manifest')
    other = record(source['other_payload_sha256'])
    notices = ['LICENSE-APACHE', 'LICENSE-MIT', 'THIRD_PARTY_NOTICES.md']
    require(set(other) == set(notices + ['README.md', 'pyproject.toml', 'src/' + namespace + 'GUIDE.md']) | set(guides),
            'unsupported-services-payload-claims')
    for location, digest in other.items():
        pin({'path': location, 'sha256': digest})
        if location == 'pyproject.toml':
            claims[location] = {'sha256': digest, 'verification': 'source_not_shipped'}
        else:
            member = (prefix + '/licenses/' + location if location in notices else
                      namespace + 'GUIDE.md' if location == 'README.md' else location[len('src/'):])
            checked_member(files, member, digest)
    docs[namespace + 'GUIDE.md'] = sha(files[namespace + 'GUIDE.md'])
    return {
        'identity': {'name': headers['Name'], 'version': headers['Version']},
        'source_files': sources, 'configuration_docs': docs, 'build_dependencies': {},
        'retained_source_claims': claims,
        # Stable across versions; retain subpaths so duplicate basenames cannot collide.
        'notices': {name[len(prefix + '/licenses/'):]: sha(value) for name, value in files.items()
                    if name.startswith(prefix + '/licenses/')},
        'package_requirements': {'requires_python': headers.get('Requires-Python'),
                                 'requires_dist': sorted(headers.get_all('Requires-Dist', [])),
                                 'license_expression': headers.get('License-Expression')},
    }


def services_descriptions(data, receipt):
    root = parse(data)
    exact(receipt, ['schema_version', 'scope', 'wheel_sha256', 'runtime_constraints_sha256',
                    'schemas_sha256', 'profiles', 'status'])
    exact(root, ['schema_version', 'wheel_sha256', 'profiles', 'descriptions'])
    require(type(receipt['schema_version']) is int and receipt['schema_version'] == 1
            and type(root['schema_version']) is int and root['schema_version'] == 1
            and receipt['scope'] == 'installed_services_description_capture'
            and receipt['status'] == 'passed', 'unsupported-services-capture')
    require(root['wheel_sha256'] == receipt['wheel_sha256'] and sha(data) == receipt['schemas_sha256'],
            'services-capture-binding-mismatch')
    exact(root['profiles'], ['profiles'])
    require(isinstance(root['profiles']['profiles'], list) and root['profiles']['profiles']
            and isinstance(root['descriptions'], list) and isinstance(receipt['profiles'], list),
            'invalid-services-profile-list')
    published, tools_by_profile = {}, {}
    for row in root['profiles']['profiles']:
        exact(row, ['profile', 'operations'])
        name = row['profile']
        require(isinstance(name, str) and NAME.fullmatch(name) and name not in published,
                'invalid-or-duplicate-profile')
        require(isinstance(row['operations'], list) and row['operations'], 'missing-profile-tools')
        declared, operations = {}, set()
        for item in row['operations']:
            exact(item, ['operation', 'tool', 'effect'])
            require(all(isinstance(item[k], str) and NAME.fullmatch(item[k]) for k in ['operation', 'tool'])
                    and item['effect'] in ['read', 'write'] and item['tool'] not in declared
                    and item['operation'] not in operations, 'invalid-or-duplicate-operation')
            declared[item['tool']] = item
            operations.add(item['operation'])
        published[name], tools_by_profile[name] = row, declared
    require(all(isinstance(name, str) for name in receipt['profiles'])
            and len(receipt['profiles']) == len(published) and set(receipt['profiles']) == set(published),
            'profile-receipt-mismatch')
    configs, operations = {}, {}
    for row in root['descriptions']:
        exact(row, ['profile', 'configuration_sha256', 'exit_code', 'description'])
        name = row['profile']
        require(isinstance(name, str) and name in published and name not in configs
                and type(row['exit_code']) is int and row['exit_code'] == 0,
                'duplicate-or-failed-schema-capture')
        pin({'path': name, 'sha256': row['configuration_sha256']})
        configs[name] = row['configuration_sha256']
        description = row['description']
        exact(description, ['profile', 'tools'])
        require(description['profile'] == name and isinstance(description['tools'], list),
                'schema-config-profile-mismatch')
        seen = set()
        for tool in description['tools']:
            record(tool)
            tool_name = tool['name']
            require(isinstance(tool_name, str) and tool_name in tools_by_profile[name] and tool_name not in seen,
                    'invalid-or-duplicate-tool')
            declared = tools_by_profile[name][tool_name]
            require(tool['operation'] == declared['operation'] and tool['effect'] == declared['effect']
                    and isinstance(tool['description'], str), 'tool-declaration-mismatch')
            record(tool['inputSchema'])
            seen.add(tool_name)
            operations[name + '/' + tool_name] = tool
        require(seen == set(tools_by_profile[name]), 'incomplete-services-description')
    require(set(configs) == set(published), 'incomplete-services-capture')
    return operations, published, configs


def npm_facts(files):
    package = parse(files['package.json'])
    require(package['name'] == '@rkat/toolkit-activepieces' and isinstance(package['version'], str), "unexpected-npm-identity")
    build = parse(files['dist/BUILD-MANIFEST.json'])
    require(type(build['schema_version']) is int and build['schema_version'] == 1, "unsupported-build-manifest")
    checked_member(files, 'vendor/SOURCE-MANIFEST.json', build['source_manifest_sha256'])
    checked_member(files, 'dist/BUILD-DEPENDENCY-LOCK.json', build['package_lock_sha256'])
    checked_member(files, 'dist/donor-factory.js', build['bundle_sha256'])
    source = parse(files['vendor/SOURCE-MANIFEST.json'])
    require(type(source['schema_version']) is int and source['schema_version'] == 1
            and source['revision'] == build['upstream_revision'], "source-revision-mismatch")
    sources = {}
    for item in source['files']:
        name = item['path']
        require(name not in sources, "duplicate-donor-source")
        checked_member(files, 'vendor/activepieces/' + name, item['sha256'])
        sources[name] = item
    require(sources and {'vendor/activepieces/' + p for p in sources} == {p for p in files if p.startswith('vendor/activepieces/')},
            'incomplete-donor-source-manifest')
    notices = parse(files['licenses/BUNDLED-PACKAGES.json'])
    require(type(notices['schema_version']) is int and notices['schema_version'] == 1
            and isinstance(notices['packages'], list) and notices['packages'], "unsupported-notice-manifest")
    seen = set()
    for item in notices['packages']:
        require(item['package_path'] not in seen and item['notices'], "missing-or-duplicate-package-notice")
        seen.add(item['package_path'])
        checked_member(files, item['retained_package_json'], item['package_json_sha256'])
        for notice in item['notices']:
            checked_member(files, notice['path'], notice['sha256'])
    build_lock = parse(files['dist/BUILD-DEPENDENCY-LOCK.json'])
    require(build_lock['lockfileVersion'] == 3, "unsupported-npm-lock")
    return {
        'identity': {key: package[key] for key in ['name', 'version']},
        'source_files': sources,
        'build_dependencies': {k: v for k, v in record(build_lock['packages']).items() if k},
        'configuration_docs': {'README.md': sha(files['README.md'])},
        'package_requirements': {k: package.get(k) for k in ['engines', 'dependencies', 'optionalDependencies', 'peerDependencies']},
    }


def descriptions(data, family, receipt):
    root = parse(data)
    configs, profiles = {}, {}
    if family == 'agno-wheel':
        profiles = record(root)
        require(set(profiles) == set(receipt['console_profiles'])
                and len(profiles) == len(receipt['console_profiles']), "profile-receipt-mismatch")
    else:
        for row in root['schemas']:
            name = row['profile']
            require(name not in profiles and type(row['status']) is int and row['status'] == 0, "duplicate-or-failed-schema-capture")
            require(row['config']['profile'] == name, "schema-config-profile-mismatch")
            profiles[name] = row['description']
            configs[name] = sha(encoded(row['config']))
        require(len(profiles) == receipt['installed_profiles_with_schema_validation'], "profile-receipt-mismatch")
    operations = {}
    for name, profile in profiles.items():
        require(isinstance(name, str) and NAME.fullmatch(name) and profile['profile'] == name, "invalid-profile-identity")
        require(isinstance(profile['tools'], list) and profile['tools'], "missing-profile-tools")
        for tool in profile['tools']:
            tool_name = tool['name']
            require(isinstance(tool_name, str) and NAME.fullmatch(tool_name), "invalid-tool-identity")
            key = name + '/' + tool_name
            require(key not in operations, "duplicate-tool-identity")
            record(tool['parameters' if family == 'agno-wheel' else 'inputSchema'])
            operations[key] = tool
    return operations, {name: {k: v for k, v in row.items() if k != 'tools'} for name, row in profiles.items()}, configs


def file_map(value):
    """Validate the retained official {path: {sha256, bytes}} format."""
    require(0 < len(record(value)) <= MAX_MEMBERS, 'invalid-file-map')
    for name, item in value.items():
        relative(name)
        exact(item, ['sha256', 'bytes'])
        pin({'path': name, 'sha256': item['sha256']})
        require(type(item['bytes']) is int and 0 <= item['bytes'] <= MAX_FILE, 'invalid-file-size')
    return value


def source_tar(data, revision):
    """Hash the existing GitHub source archive without extracting it."""
    require(isinstance(revision, str) and re.fullmatch(r'[0-9a-f]{40}', revision), 'invalid-source-revision')
    prefix, result, seen, total = 'github-mcp-server-' + revision, {}, set(), 0
    with bounded_tar(data) as archive:
        for member in archive:
            name = member.name.rstrip('/') if member.isdir() else member.name
            relative(name)
            require(name not in seen and len(seen) < MAX_MEMBERS, 'duplicate-or-excess-archive-member')
            seen.add(name)
            require(name == prefix or name.startswith(prefix + '/'), 'source-archive-prefix-mismatch')
            if member.isdir():
                continue
            require(member.isfile() and 0 <= member.size <= MAX_MEMBER
                    and total + member.size <= MAX_TOTAL, 'unsupported-source-archive-member')
            with archive.extractfile(member) as stream:
                content = stream.read(MAX_MEMBER + 1)
            require(len(content) == member.size, 'archive-member-size-mismatch')
            result[name[len(prefix) + 1:]] = sha(content)
            total += member.size
    require(result, 'empty-source-archive')
    return result


def official_github(root, spec, component, files):
    exact(spec['provenance'], ['source_archive', 'source_audit', 'build_receipt'])
    provenance = {key: read_pin(root, ref) for key, ref in spec['provenance'].items()}
    audit, build = parse(provenance['source_audit']), parse(provenance['build_receipt'])
    revision = component['source_revision']
    require(audit['source_revision'] == build['source_revision'] == revision
            and audit['source_archive_sha256'] == spec['provenance']['source_archive']['sha256'],
            'source-provenance-mismatch')
    source = source_tar(provenance['source_archive'], revision)
    recorded = {}
    for row in audit['source_files']:
        pin(row)
        require(row['path'] not in recorded, 'duplicate-source-file')
        recorded[row['path']] = row['sha256']
    require(source == recorded, 'source-archive-audit-mismatch')
    artifact = component['artifact']
    require(artifact == 'github/github-mcp-server'
            and sha(files[artifact]) == component['artifact_sha256'] == build['binary_sha256'],
            'binary-provenance-mismatch')
    dependencies = {}
    for row in audit['binary_dependencies']:
        name = row['path']
        require(isinstance(name, str) and name and name not in dependencies, 'duplicate-or-invalid-build-dependency')
        dependencies[name] = {'version': row['version'], 'sum': row['sum']}
    if spec['platform'] == 'macos-arm64':
        require(audit['binary_sha256'] == build['binary_sha256']
                and build['source_audit_sha256'] == spec['provenance']['source_audit']['sha256'],
                'build-source-audit-mismatch')
    else:
        require(build['status'] == 'passed_offline_crossbuild_only' and build['target'] == 'linux/amd64'
                and build['cgo_enabled'] is False, 'unsupported-crossbuild-record')
        linked = {}
        for row in build['linked_dependencies']:
            require(isinstance(row, list) and len(row) == 3 and row[0] not in linked, 'invalid-linked-dependency')
            linked[row[0]] = {'version': row[1], 'sum': row[2]}
        require(linked == dependencies, 'crossbuild-dependency-mismatch')
    notices = {}
    for row in audit['notices']:
        name = 'github/' + str(relative(row['retained']))
        require(name.startswith('github/notices/') and name not in notices, 'invalid-source-notice')
        checked_member(files, name, row['sha256'])
        notices[name] = row['sha256']
    require(set(notices) == {p for p in files if p.startswith('github/notices/')}, 'incomplete-source-notices')
    installed = {p[len('github/'):]: {'sha256': sha(data), 'bytes': len(data)} for p, data in files.items()
                 if p == artifact or p.startswith('github/notices/')}
    return {'identity': {'name': 'github-mcp-server', 'version': revision},
            'artifact_sha256': sha(files[artifact]), 'source_files': source, 'build_dependencies': dependencies,
            'runtime_dependencies': {}, 'package_requirements': {'runtime_profile': component['runtime_profile']},
            'expected_installed': installed}


def official_playwright(root, spec, component, files):
    exact(spec['provenance'], ['source_match'])
    source_match = parse(read_pin(root, spec['provenance']['source_match']))
    require(component['package_json'] == 'playwright/package.json'
            and component['package_lock'] == 'playwright/package-lock.json'
            and component['closure'] == 'playwright/closure.json', 'unsupported-playwright-layout')
    package = parse(files[component['package_json']])
    lock = parse(files[component['package_lock']])
    closure = parse(files[component['closure']])
    require(type(lock['lockfileVersion']) is int and lock['lockfileVersion'] == 3, 'unsupported-npm-lock')
    require(closure['package_json_sha256'] == sha(files[component['package_json']])
            and closure['package_lock_sha256'] == sha(files[component['package_lock']]), 'closure-input-mismatch')
    require(isinstance(closure['packages'], list) and len(closure['packages']) == 3, 'unsupported-playwright-closure')
    installed, versions, archives, source, requirements = {}, {}, {}, {}, {}
    for row in closure['packages']:
        name, location = row['id'], row['install_path']
        require(name in ['@playwright/mcp', 'playwright', 'playwright-core'] and name not in versions
                and location == 'node_modules/' + name, 'unsupported-playwright-package')
        archive = str(relative(row['path']))
        require(archive.startswith('playwright/archives/'), 'invalid-playwright-archive-path')
        checked_member(files, archive, row['sha256'])
        data = files[archive]
        integrity = 'sha512-' + base64.b64encode(hashlib.sha512(data).digest()).decode()
        locked = lock['packages'][location]
        require(type(row['bytes']) is int and row['bytes'] == len(data)
                and locked['integrity'] == row['integrity'] == integrity
                and locked['version'] == row['version'] and locked['resolved'] == row['url'], 'npm-closure-lock-mismatch')
        payload = archive_payload(data, 'activepieces-npm')
        metadata = parse(payload['package.json'])
        require(metadata['name'] == name and metadata['version'] == row['version']
                and metadata.get('engines') == row['engines'] == locked.get('engines')
                and metadata.get('dependencies') == locked.get('dependencies')
                and metadata.get('optionalDependencies') == locked.get('optionalDependencies')
                and type(row['payload_files']) is int and row['payload_files'] == len(payload), 'npm-closure-package-mismatch')
        versions[name], archives[archive] = row['version'], row['sha256']
        requirements[name] = {k: metadata.get(k) for k in ['engines', 'dependencies', 'optionalDependencies']}
        for path, content in payload.items():
            target = location + '/' + path
            require(target not in installed, 'duplicate-installed-payload')
            installed[target] = {'sha256': sha(content), 'bytes': len(content)}
            source[target] = sha(content)
    require(set(lock['packages']) == {''} | {'node_modules/' + k for k in versions}
            and package['dependencies'] == lock['packages']['']['dependencies'] == {'@playwright/mcp': versions['@playwright/mcp']},
            'npm-closure-dependency-mismatch')
    require(set(archives) == {p for p in files if p.startswith('playwright/archives/')}, 'incomplete-playwright-archives')
    exact(source_match, ['index.js', 'cli.js', 'LICENSE'])
    for name, row in source_match.items():
        exact(row, ['sha256', 'equal'])
        require(row['equal'] is True and source['node_modules/@playwright/mcp/' + name] == row['sha256'], 'wrapper-source-match-mismatch')
    notices = {p: sha(data) for p, data in files.items() if p.startswith('playwright/notices/')}
    for path, digest in notices.items():
        name = path[len('playwright/notices/'):].replace('@playwright__mcp/', '@playwright/mcp/', 1)
        require(source.get('node_modules/' + name) == digest, 'npm-notice-payload-mismatch')
    require(set(notices) == {'playwright/notices/@playwright__mcp/LICENSE'}
            | {'playwright/notices/' + name + '/' + notice for name in ['playwright', 'playwright-core']
               for notice in ['LICENSE', 'NOTICE', 'ThirdPartyNotices.txt']}, 'incomplete-playwright-notices')
    return {'identity': {'name': '@playwright/mcp', 'version': versions['@playwright/mcp']},
            'artifact_sha256': sha(encoded(archives)), 'source_files': source, 'build_dependencies': {},
            'runtime_dependencies': {p: v for p, v in lock['packages'].items() if p},
            'package_requirements': requirements, 'expected_installed': installed, 'expected_versions': versions}


def official_selection(root, reference, spec):
    exact(spec, ['schema_version', 'family', 'component', 'platform', 'edition', 'contents', 'delivery', 'receipt', 'schemas', 'provenance'])
    require(type(spec['schema_version']) is int and spec['schema_version'] == 1
            and spec['component'] in ['github', 'playwright'] and spec['platform'] in ['macos-arm64', 'linux-x86_64']
            and spec['edition'] in ['initial', 'restore'], 'unsupported-official-selection')
    data = {key: read_pin(root, spec[key]) for key in ['contents', 'delivery', 'receipt', 'schemas']}
    contents, delivery, receipt = (parse(data[key]) for key in ['contents', 'delivery', 'receipt'])
    require(type(contents['format']) is int and contents['format'] == 1
            and delivery['format'] == 'meerkat-toolkit.official-consumers.v1', 'unsupported-official-bundle')
    manifest = file_map(contents['files'])
    require(receipt['contents_sha256'] == spec['contents']['sha256']
            and receipt['delivery_sha256'] == spec['delivery']['sha256']
            and manifest['delivery.json'] == {'sha256': sha(data['delivery']), 'bytes': len(data['delivery'])},
            'official-receipt-bundle-mismatch')
    bundle = PurePosixPath(spec['contents']['path']).parent
    require(PurePosixPath(spec['delivery']['path']) == bundle / 'delivery.json', 'official-delivery-location-mismatch')
    name = spec['component']
    components = [c for c in delivery['components'] if c['catalog_id'] == name]
    require(len(components) == 1, 'missing-or-duplicate-official-component')
    component = components[0]
    files, total = {}, 0
    for path, row in manifest.items():
        if path.startswith(name + '/'):
            total += row['bytes']
            require(total <= MAX_TOTAL, 'official-bundle-size-limit')
            files[path] = read_pin(root, {'path': str(bundle / path), 'sha256': row['sha256']})
            require(len(files[path]) == row['bytes'], 'official-bundle-size-mismatch')
    facts = official_github(root, spec, component, files) if name == 'github' else official_playwright(root, spec, component, files)
    installation = receipt['installations'][name + '-' + spec['edition']]
    require(file_map(installation['files']) == facts.pop('expected_installed'), 'recorded-installation-payload-mismatch')
    if name == 'playwright':
        require(installation['versions'] == facts.pop('expected_versions'), 'recorded-installation-version-mismatch')
    if spec['platform'] == 'macos-arm64':
        require(receipt['status'] == 'passed' and receipt['bundle_unchanged_during_protocol'] is True, 'unsupported-macos-run-record')
        consumers = receipt['protocol']['consumers']
    else:
        require(receipt['status'] in ['passed', 'partial_protocol_failure'] and receipt['bundle_unchanged'] is True,
                'unsupported-linux-run-record')
        consumers = receipt['protocol']['outcomes']
    selected = [row for row in consumers if row['family'] == name and row['edition'] == spec['edition']]
    require(len(selected) == 1, 'missing-or-duplicate-run-outcome')
    outcome = selected[0]
    # The old macOS rows predate an explicit per-consumer status. Only its
    # absence permits the legacy inference; explicit failures remain failures.
    status = outcome.get('status', 'passed') if spec['platform'] == 'macos-arm64' else outcome['status']
    require(status in ['passed', 'failed'], 'unsupported-component-outcome')
    require(status != 'passed' or not any(outcome.get(key) for key in ['error', 'error_type', 'traceback']),
            'contradictory-component-outcome')
    capture = parse(data['schemas'])
    require(capture['result_type'] == 'complete' and capture['next_cursor'] is None
            and isinstance(capture['tools'], list) and 0 < len(capture['tools']) <= MAX_MEMBERS, 'incomplete-tools-capture')
    operations = {}
    for tool in capture['tools']:
        tool_name = tool['name']
        require(isinstance(tool_name, str) and NAME.fullmatch(tool_name)
                and name + '/' + tool_name not in operations, 'duplicate-or-invalid-tool-name')
        record(tool['input_schema'])
        operations[name + '/' + tool_name] = tool
    names = [tool['name'] for tool in capture['tools']]
    if status == 'passed':
        require(sorted(outcome['tools']) == sorted(names), 'run-tool-names-mismatch')
    require(isinstance(component['selected_operations'], list) and component['selected_operations']
            and set(component['selected_operations']).issubset(names), 'selected-operation-missing')
    template = component['configuration_template']
    require(template == name + '/mcp.template.json' and template in files, 'missing-configuration-template')
    facts.update({
        'selection': reference, 'family': spec['family'],
        'inputs': {key: spec[key] for key in ['contents', 'delivery', 'receipt', 'schemas', 'provenance']},
        'payloads': {p: sha(value) for p, value in files.items()},
        'notices': {p: sha(value) for p, value in files.items() if p.startswith(name + '/notices/')},
        # Compare the selected delivery record, not only its tool/runtime subset.
        # Installer-relevant secret, exposure and platform declarations can change
        # while the executable and captured tool schemas remain byte-identical.
        'profiles': {name: component},
        'delivery_metadata': {key: delivery[key] for key in ['role', 'scope'] if key in delivery},
        'operations': operations, 'recorded_configuration_hashes': {name: sha(files[template])},
        'configuration_docs': {p: sha(value) for p, value in files.items() if p.endswith('.md') and '/notices/' not in p},
        'verification_observations': {'platform': spec['platform'], 'edition': spec['edition'],
            'run_status': receipt['status'], 'component_status': status,
            'outcome_sha256': sha(encoded(outcome)), 'schema_association': 'separately_pinned_capture_not_session_attestation',
            'installed_payload_scope': 'retained_receipt_map_not_current_installation'},
    })
    require(facts['notices'], 'missing-artifact-notices')
    return facts


def selection(root, reference):
    spec = parse(read_pin(root, reference))
    if record(spec).get('family') == 'official-consumer':
        return official_selection(root, reference, spec)
    exact(spec, ['schema_version', 'family', 'artifact', 'receipt', 'evidence_manifest', 'schemas', 'runtime_dependencies'])
    require(type(spec['schema_version']) is int and spec['schema_version'] == 1
            and spec['family'] in ['agno-wheel', 'services-wheel', 'activepieces-npm'], "unsupported-selection-format")
    family = spec['family']
    evidence = parse(read_pin(root, spec['evidence_manifest']))
    evidence = record(evidence['files'] if family in ['agno-wheel', 'services-wheel'] else evidence)
    evidence_root = PurePosixPath(spec['evidence_manifest']['path']).parent
    data = {}
    for role in ['artifact', 'receipt', 'schemas', 'runtime_dependencies']:
        ref = pin(spec[role])
        try:
            rel = str(PurePosixPath(ref['path']).relative_to(evidence_root))
        except ValueError:
            raise CheckError('input-outside-evidence-manifest') from None
        require(evidence.get(rel) == ref['sha256'], 'input-not-bound-by-evidence-manifest')
        data[role] = read_pin(root, ref)
    receipt = parse(data['receipt'])
    artifact_key = 'wheel_sha256' if family in ['agno-wheel', 'services-wheel'] else 'archive_sha256'
    require(receipt[artifact_key] == spec['artifact']['sha256'], 'receipt-artifact-mismatch')
    files = archive_payload(data['artifact'], family)
    facts = (wheel_facts(files) if family == 'agno-wheel' else
             services_facts(files) if family == 'services-wheel' else npm_facts(files))
    if family in ['agno-wheel', 'services-wheel']:
        require(receipt['runtime_constraints_sha256'] == spec['runtime_dependencies']['sha256'], 'receipt-dependency-mismatch')
        dependencies = {}
        for line in data['runtime_dependencies'].decode('utf-8').splitlines():
            if not line.strip() or line.lstrip().startswith('#'):
                continue
            match = re.fullmatch(r'([A-Za-z0-9_.-]+)==([A-Za-z0-9_.+!-]+)', line)
            require(match is not None, 'unsupported-python-pin')
            name = re.sub(r'[-_.]+', '-', match[1]).lower()
            require(name not in dependencies, 'duplicate-python-pin')
            dependencies[name] = match[2]
    else:
        dependencies = record(parse(data['runtime_dependencies']))
        for name, row in dependencies.items():
            require(name.startswith('node_modules/') and isinstance(row.get('version'), str)
                    and isinstance(row.get('integrity'), str), 'invalid-runtime-dependency-record')
        require(len(dependencies) == receipt['runtime_dependency_packages'], 'runtime-dependency-count-mismatch')
    operations, profiles, configs = (services_descriptions(data['schemas'], receipt) if family == 'services-wheel'
                                    else descriptions(data['schemas'], family, receipt))
    if family == 'services-wheel':
        facts['capture_scope'] = {
            'scope': receipt['scope'], 'status': receipt['status'],
            'unverified_source_files': ['pyproject.toml'],
            'canonical_source_paths_and_guide_projections': 'retained_claims_not_independently_reproduced',
        }
    facts.update({
        'selection': reference, 'family': family, 'inputs': {k: spec[k] for k in spec if isinstance(spec[k], dict)},
        'artifact_sha256': spec['artifact']['sha256'],
        'payloads': {name: sha(value) for name, value in files.items()},
        'notices': facts['notices'] if family == 'services-wheel' else
                   {name: sha(value) for name, value in files.items()
                    if 'license' in name.lower() or 'notice' in name.lower()},
        'runtime_dependencies': dependencies, 'profiles': profiles, 'operations': operations,
        'recorded_configuration_hashes': configs,
    })
    require(facts['notices'], 'missing-artifact-notices')
    return facts


def changed_paths(before, after, prefix=''):
    if encoded(before) == encoded(after):
        return []
    if isinstance(before, dict) and isinstance(after, dict):
        result = []
        for key in sorted(set(before) | set(after)):
            path = prefix + '/' + key.replace('~', '~0').replace('/', '~1')
            result.extend([path] if key not in before or key not in after else changed_paths(before[key], after[key], path))
        return result
    return [prefix or '/']


def delta(before, after):
    return {
        'added': sorted(set(after) - set(before)), 'removed': sorted(set(before) - set(after)),
        'changed': {key: {'before_sha256': sha(encoded(before[key])), 'after_sha256': sha(encoded(after[key])),
                         'changed_fields': changed_paths(before[key], after[key])}
                    for key in sorted(set(before) & set(after)) if encoded(before[key]) != encoded(after[key])},
        'unchanged_count': sum(encoded(before[key]) == encoded(after[key]) for key in set(before) & set(after)),
    }


def proposal(root, reference, before, after):
    value = parse(read_pin(root, reference))
    action = value.get('action')
    fields = ['schema_version', 'action', 'from', 'reason', 'migration_guidance']
    exact(value, fields + (['replacement'] if action == 'replace' else []))
    require(type(value['schema_version']) is int and value['schema_version'] == 1
            and action in ['deprecate', 'replace'], 'unsupported-proposal')
    for key in ['reason', 'migration_guidance']:
        require(isinstance(value[key], str) and bool(value[key].strip()), 'missing-proposal-guidance')
    for key, selected in [('from', before)] + ([('replacement', after)] if action == 'replace' else []):
        exact(value[key], ['artifact_sha256', 'profile'])
        require(value[key]['artifact_sha256'] == selected['artifact_sha256']
                and value[key]['profile'] in selected['profiles'], 'proposal-target-mismatch')
    if action == 'replace':
        require(value['from'] != value['replacement'], 'replacement-is-identical')
    return {'reference': reference, 'action': action, 'from': value['from'],
            'replacement': value.get('replacement'), 'guidance_sha256': sha(encoded({k: value[k] for k in ['reason', 'migration_guidance']})),
            'disposition': 'proposal_only_no_support_change_or_retargeting'}


def compare(root, before_ref, after_ref, proposal_ref=None):
    before, after = selection(root, before_ref), selection(root, after_ref)
    require(before['family'] == after['family'] and before['identity']['name'] == after['identity']['name'], 'incomparable-package-identities')
    groups = ['payloads', 'source_files', 'notices', 'runtime_dependencies', 'build_dependencies',
              'profiles', 'operations', 'recorded_configuration_hashes', 'configuration_docs', 'package_requirements']
    if before['family'] == 'official-consumer':
        groups += ['verification_observations', 'delivery_metadata']
    if before['family'] == 'services-wheel':
        groups.append('retained_source_claims')
    report = {
        'schema_version': 1, 'result': 'comparison_only_maintainer_review_required',
        'before': {k: before[k] for k in ['selection', 'family', 'identity', 'artifact_sha256', 'inputs']},
        'after': {k: after[k] for k in ['selection', 'family', 'identity', 'artifact_sha256', 'inputs']},
        'same_display_version_different_artifact': before['identity']['version'] == after['identity']['version']
                                                 and before['artifact_sha256'] != after['artifact_sha256'],
        'changes': {group: delta(before[group], after[group]) for group in groups},
        'proposal': proposal(root, proposal_ref, before, after) if proposal_ref else None,
        'checks_prove': ['Explicit local input digests match.',
                         'Selected artifact, receipt, schemas and runtime pins are members of the supplied evidence manifest.',
                         'The supplied execution receipt names this artifact digest and recorded profile/dependency scope.',
                         'Packaged source/notice bindings and bounded archive structure match the selected bytes.',
                         'Reported differences are structural observations of the retained inputs.'],
        'not_proven': ['Receipt authorship, maintainer approval, support promotion, legal sufficiency or semantic compatibility.',
                       'Current provider health or schema; schemas are retained observations, not fresh provider discovery.',
                       'Installed dependency integrity, new conformance runs, authentication, native attachment or task completion.',
                       'Canonical service counts, deployment migration, rollback or safe replacement of an existing connection.'],
    }
    if before['family'] == 'services-wheel':
        for side, selected in [('before', before), ('after', after)]:
            report[side]['capture_scope'] = selected['capture_scope']
        report['checks_prove'][2] = 'The supplied description-capture receipt binds the wheel, complete profile/tool capture and runtime-constraint bytes.'
        report['not_proven'] += [
            'Authenticity or execution of the supplied installed-description capture; no candidate code is executed here.',
            'Configuration hashes are recorded claims; configuration bytes and installed payloads are not re-read by this command.',
            'Absent pyproject.toml bytes, canonical source path identity or reproduction of claimed guide projections.']
    if before['family'] == 'official-consumer':
        for side, selected in [('before', before), ('after', after)]:
            report[side]['verification_observations'] = selected['verification_observations']
        report['checks_prove'] = [
            'Explicit local input digests and selected component CONTENTS payload/notice bindings match.',
            'The retained run receipt binds the selected bundle and its recorded installation payload map matches retained bytes.',
            'GitHub source archive/audit and build-receipt claims, or Playwright npm closure/lock and three-file source-match claims, agree with selected bytes.',
            'Separately pinned tools captures are structurally complete; successful run records agree on tool names only.',
            'Per-component failed or passed outcomes are retained historical observations; comparison is not conformance acceptance.',
            'Reported differences are structural observations of the retained inputs.']
        report['not_proven'] += [
            'A cryptographic link between the historical tools capture and the recorded execution session.',
            'An independent rebuild, upstream checkout identity, current installed payloads or a successful browser run from tool listing alone.']
    return report


def readable(report):
    lines = ['# Library update observations', '', 'Result: comparison only; maintainer review required.', '']
    for side in ['before', 'after']:
        row = report[side]
        lines.append(f"{side.title()}: {json.dumps(row['identity'], sort_keys=True)}; artifact SHA-256 {row['artifact_sha256']}")
        if 'verification_observations' in row:
            observed = row['verification_observations']
            lines.append('Recorded ' + observed['platform'] + ' ' + observed['edition'] + ' outcome: '
                         + observed['component_status'] + '; overall run: ' + observed['run_status'] + '.')
        if 'capture_scope' in row:
            lines.append('Recorded description capture: installed_services_description_capture; '
                         'this comparison does not execute or authenticate that capture. '
                         'Unshipped pyproject.toml and canonical source/guide projection claims remain unverified.')
    lines += ['', 'Same display version with different bytes: ' + str(report['same_display_version_different_artifact']).lower(), '']
    for group, change in report['changes'].items():
        lines.append(f"## {group}")
        lines.append('')
        lines.append(f"Added: {len(change['added'])}; removed: {len(change['removed'])}; changed: {len(change['changed'])}; unchanged: {change['unchanged_count']}.")
        for kind in ['added', 'removed', 'changed']:
            for key in change[kind]:
                lines.append('- ' + kind + ': ' + json.dumps(key, ensure_ascii=True))
        lines.append('')
    lines += ['## Proposal', '', json.dumps(report['proposal'], sort_keys=True, ensure_ascii=True), '']
    for title, key in [('What was checked', 'checks_prove'), ('What remains unproven', 'not_proven')]:
        lines += ['## ' + title, ''] + ['- ' + text for text in report[key]] + ['']
    return '\n'.join(lines)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--root', required=True, type=Path)
    for side in ['before', 'after']:
        parser.add_argument('--' + side, required=True)
        parser.add_argument('--' + side + '-sha256', required=True)
    parser.add_argument('--proposal')
    parser.add_argument('--proposal-sha256')
    parser.add_argument('--format', choices=['json', 'markdown'], default='json')
    args = parser.parse_args()
    try:
        require(bool(args.proposal) == bool(args.proposal_sha256), 'proposal-needs-path-and-digest')
        report = compare(args.root, {'path': args.before, 'sha256': args.before_sha256},
                         {'path': args.after, 'sha256': args.after_sha256},
                         {'path': args.proposal, 'sha256': args.proposal_sha256} if args.proposal else None)
        print(json.dumps(report, sort_keys=True, ensure_ascii=True, allow_nan=False, indent=2) if args.format == 'json' else readable(report))
        return 0
    except (OSError, ValueError, KeyError, TypeError, IndexError, AttributeError, EOFError,
            csv.Error, zipfile.BadZipFile, tarfile.TarError, zlib.error, RuntimeError) as exc:
        print('library-update: ' + (str(exc) if isinstance(exc, CheckError) else 'invalid-or-unreadable-selected-evidence'), file=sys.stderr)
        return 2


if __name__ == '__main__':
    sys.exit(main())
