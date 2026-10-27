"""Fixed fresh-VM prerequisites, executed by the GCP guest agent as root.

This source is part of the declared guided deployment adapter. It performs no
application configuration or start and publishes no application readiness claim.
The ordinary destination controller still checks all installation prerequisites.
"""
BOOTSTRAP_SCRIPT = """#!/usr/bin/env bash
set -euo pipefail
export DEBIAN_FRONTEND=noninteractive
apt-get update
apt-get install -y python3 python3-venv ca-certificates dbus-user-session
for attempt in $(seq 1 60); do
  if id mobkit >/dev/null 2>&1; then
    loginctl enable-linger mobkit
    exit 0
  fi
  sleep 2
done
echo 'The installation user is not ready; recheck the owned VM before installing.' >&2
exit 1
"""

# Kept separate from the historical base script so existing cloud receipts keep
# their exact bootstrap bytes. Only granted, known commands select these tools.
RUNTIME_SETUP = r'''
import hashlib, io, os, platform, posixpath, stat, tarfile, urllib.request, zipfile
from pathlib import Path, PurePosixPath
ROOT_UID = 0
NODE_VERSION = '22.23.3'
UV_VERSION = '0.12.23'
NODE_HASHES = {'x86_64': 'df450af89261115ef9f9e3830c3eeb2cc9213b63c720b1af623cb5dcbe2e02de', 'aarch64': 'a44aeb94849a299b22df10b9e622ec2f605c2183501bc40590705131de7c740f'}
UV_WHEELS = {
 'x86_64': ('e5/83/85a6c63c24905af4924fddb11a499b934913f59a134248367a1ef1a4716f/uv-0.12.23-py3-none-manylinux_2_17_x86_64.manylinux2014_x86_64.whl', '565c6e2874dbeae86c02f3dea97255e878fec672659a73d4930c6b93fcab2fff'),
 'aarch64': ('df/50/84d0424b63718ac6f277c0b77206751e078a0adea16813320508a033e718/uv-0.12.23-py3-none-manylinux_2_28_aarch64.whl', '9346c741175c7fc0d380f30fcfcb8d5e13f5f900b30536230c8d34208524e47a')}

def download(url, digest):
    with urllib.request.urlopen(url, timeout=60) as response:
        if response.geturl().split('/', 3)[:3] != url.split('/', 3)[:3]:
            raise ValueError('Download changed its HTTPS host.')
        data = response.read(64 * 1024 * 1024 + 1)
    if len(data) > 64 * 1024 * 1024 or hashlib.sha256(data).hexdigest() != digest:
        raise ValueError('Pinned checksum or size check failed.')
    return data

def node_files(data, prefix):
    files, links, names, size = {}, {}, set(), 0
    with tarfile.open(fileobj=io.BytesIO(data), mode='r|xz') as archive:
        for member in archive:
            parts = PurePosixPath(member.name).parts
            normalized = '/'.join(parts)
            if (not parts or parts[0] != prefix or '..' in parts or normalized != member.name.rstrip('/')
                    or any(ord(c) < 32 for c in member.name) or normalized in names or len(names) >= 10000):
                raise ValueError('Unsafe prerequisite archive path.')
            names.add(normalized)
            name = '/'.join(parts[1:])
            if member.isdir():
                if member.size: raise ValueError('Unsafe prerequisite directory.')
                continue
            if not name or not (member.isfile() or member.issym()):
                raise ValueError('Unsupported archive member.')
            if member.issym():
                target = posixpath.normpath(posixpath.join(posixpath.dirname(name), member.linkname))
                if (member.linkname.startswith('/') or target == '..' or target.startswith('../')
                        or any(ord(c) < 32 for c in member.linkname)):
                    raise ValueError('Unsafe prerequisite archive link.')
                links[name] = (member.linkname, target)
            else:
                size += member.size
                if member.size < 0 or size > 256 * 1024 * 1024:
                    raise ValueError('Expanded archive is too large.')
                content = archive.extractfile(member).read(member.size + 1)
                if len(content) != member.size: raise ValueError('Incomplete prerequisite archive.')
                files[name] = (content, 0o755 if member.mode & 0o111 else 0o644)
    for name, (_, target) in links.items():
        if target not in files: raise ValueError('Link target is not a checked file.')
    for name in (*files, *links):
        if any(str(parent) in links or str(parent) in files for parent in PurePosixPath(name).parents if str(parent) != '.'):
            raise ValueError('Conflicting archive parent.')
    if 'bin/node' not in files or not {'bin/npm', 'bin/npx'} <= links.keys():
        raise ValueError('Expected Node commands are missing.')
    return files, links

def uv_files(data):
    files = {}
    with zipfile.ZipFile(io.BytesIO(data)) as wheel:
        members = wheel.infolist()
        if len(members) > 100 or len({item.filename for item in members}) != len(members):
            raise ValueError('Unsafe uv wheel.')
        for name in ('uv', 'uvx'):
            member = wheel.getinfo('uv-' + UV_VERSION + '.data/scripts/' + name)
            if not 0 < member.file_size <= 128 * 1024 * 1024 or stat.S_ISLNK(member.external_attr >> 16):
                raise ValueError('Unsafe uv executable.')
            files['bin/' + name] = (wheel.read(member), 0o755)
    return files, {}

def owned_directory(path):
    info = path.lstat()
    if not stat.S_ISDIR(info.st_mode) or info.st_uid != ROOT_UID or info.st_mode & 0o022:
        raise ValueError('Directory is not safely root-owned.')

def install_tree(name, files, links, commands, opt=Path('/opt'), bin_dir=Path('/usr/local/bin')):
    owned_directory(opt)
    owned_directory(bin_dir.parent)
    if not bin_dir.exists(): bin_dir.mkdir(mode=0o755)
    owned_directory(bin_dir)
    target = opt / name
    for command in commands:
        path = bin_dir / command
        if path.exists() or path.is_symlink():
            info = path.lstat()
            if not stat.S_ISLNK(info.st_mode) or info.st_uid != ROOT_UID or os.readlink(path) != str(target / 'bin' / command):
                raise ValueError('Command belongs to another installation.')
    directories = {str(parent) for value in (*files, *links) for parent in PurePosixPath(value).parents if str(parent) != '.'}
    expected = set(files) | set(links) | directories
    if target.exists() or target.is_symlink():
        owned_directory(target)
        if {str(path.relative_to(target)) for path in target.rglob('*')} != expected:
            raise ValueError('Prerequisite files changed.')
        for value in expected:
            path, info = target / value, (target / value).lstat()
            if info.st_uid != ROOT_UID: raise ValueError('Prerequisite owner changed.')
            if value in links:
                if not stat.S_ISLNK(info.st_mode) or os.readlink(path) != links[value][0]: raise ValueError('Prerequisite link changed.')
            elif value in files:
                content, mode = files[value]
                if not stat.S_ISREG(info.st_mode) or info.st_nlink != 1 or stat.S_IMODE(info.st_mode) != mode or info.st_size != len(content) or path.read_bytes() != content:
                    raise ValueError('Prerequisite file changed.')
            else: owned_directory(path)
    else:
        # Exclusive creation cannot replace a directory that appears meanwhile.
        # A partial failed tree is retained and rejected on the next attempt.
        target.mkdir(mode=0o700)
        for directory in sorted(directories, key=lambda value: (value.count('/'), value)):
            (target / directory).mkdir(mode=0o755)
        for value, (content, mode) in files.items():
            path = target / value
            path.write_bytes(content)
            path.chmod(mode)
        for value, (link, _) in links.items(): (target / value).symlink_to(link)
    target.chmod(0o755)
    for command in commands:
        path = bin_dir / command
        if not path.is_symlink(): path.symlink_to(target / 'bin' / command)

def install(node, uv):
    architecture = platform.machine()
    if architecture not in NODE_HASHES: raise ValueError('Unsupported prerequisite CPU architecture.')
    if node:
        prefix = 'node-v' + NODE_VERSION + '-linux-' + {'x86_64': 'x64', 'aarch64': 'arm64'}[architecture]
        files, links = node_files(download('https://nodejs.org/dist/v' + NODE_VERSION + '/' + prefix + '.tar.xz', NODE_HASHES[architecture]), prefix)
        install_tree(prefix, files, links, ('node', 'npm', 'npx'))
    if uv:
        wheel, digest = UV_WHEELS[architecture]
        files, links = uv_files(download('https://files.pythonhosted.org/packages/' + wheel, digest))
        install_tree('mobkit-uv-' + UV_VERSION, files, links, ('uv', 'uvx'))
'''


KNOWLEDGE_SETUP = """apt-get install -y git curl
install -m 0755 -d /etc/apt/keyrings
curl --fail --silent --show-error --max-time 60 https://download.docker.com/linux/debian/gpg -o /etc/apt/keyrings/forge-docker.asc
chmod 0644 /etc/apt/keyrings/forge-docker.asc
printf 'deb [arch=%s signed-by=/etc/apt/keyrings/forge-docker.asc] https://download.docker.com/linux/debian bookworm stable\\n' "$(dpkg --print-architecture)" > /etc/apt/sources.list.d/forge-docker.list
apt-get update
apt-get install -y docker-ce docker-ce-cli containerd.io docker-buildx-plugin docker-compose-plugin
systemctl enable --now docker
"""

KNOWLEDGE_USER_SETUP = """    if ! id -nG mobkit | grep -qw docker; then
      usermod -aG docker mobkit
    fi
    runuser -u mobkit -- docker info >/dev/null
    docker compose up --help | grep -E '^ +--wait +' >/dev/null
    docker compose up --help | grep -E '^ +--wait-timeout +' >/dev/null
"""

KNOWLEDGE_MANAGER_SETUP = """    knowledge_uid=$(id -u mobkit)
    [[ "$knowledge_uid" =~ ^[0-9]+$ ]] || exit 1
    knowledge_docker_gid=$(getent group docker | awk -F: 'NR!=1 || NF!=4 || $1!="docker" || $3 !~ /^[0-9]+$/ {exit 1} {print $3} END {if(NR!=1) exit 1}')
    knowledge_manager_pid() {
      knowledge_pid=$(systemctl show --property=MainPID --value "user@$knowledge_uid.service")
      [[ "$knowledge_pid" =~ ^[0-9]+$ ]] || return 1
    }
    knowledge_manager_groups() {
      awk -v gid="$knowledge_docker_gid" '$1=="Groups:" {seen++; for(i=2;i<=NF;i++) {if($i !~ /^[0-9]+$/) bad=1; if($i==gid) found=1}} END {if(bad || seen!=1) exit 2; exit(found ? 0 : 1)}' "/proc/$knowledge_pid/status"
    }
    knowledge_manager_pid || exit 1
    if [[ "$knowledge_pid" == 0 ]]; then
      knowledge_manager_pid || exit 1
      [[ "$knowledge_pid" == 0 ]] || exit 1
      systemctl start "user@$knowledge_uid.service"
    elif knowledge_manager_groups; then
      :
    else
      knowledge_group_status=$?
      [[ "$knowledge_group_status" == 1 ]] || exit 1
      knowledge_stale_pid=$knowledge_pid
      knowledge_manager_pid || exit 1
      [[ "$knowledge_pid" == "$knowledge_stale_pid" ]] || exit 1
      systemctl restart "user@$knowledge_uid.service"
    fi
    knowledge_manager_pid || exit 1
    [[ "$knowledge_pid" != 0 ]] || exit 1
    knowledge_manager_groups || exit 1
    knowledge_verified_pid=$knowledge_pid
    knowledge_manager_pid || exit 1
    [[ "$knowledge_pid" == "$knowledge_verified_pid" ]] || exit 1
"""


def bootstrap_script(definition):
    """Select supported prerequisites from grants in the exact checked package."""
    from .remote_destination import connection_prerequisites
    commands = set(connection_prerequisites(definition)['mcp_prerequisites'])
    node = bool(commands.intersection({'node', 'npm', 'npx'}))
    uv = bool(commands.intersection({'uv', 'uvx'}))
    local_knowledge = (definition.get('knowledge') or {}).get('mode', 'none') == 'local'
    if local_knowledge:
        setup = KNOWLEDGE_SETUP
        if node or uv:
            # Encode the exact existing runtime only for this larger selection.
            # The bounded guest script retains its original checked tool logic.
            import base64
            import zlib
            encoded = base64.b64encode(zlib.compress(RUNTIME_SETUP.encode(), 9)).decode('ascii')
            setup += ("python3 -I - <<'MOBKIT_PREREQUISITES'\nimport base64,zlib\n"
                + f"exec(zlib.decompress(base64.b64decode({encoded!r})))\n"
                + f"install({node!r}, {uv!r})\nMOBKIT_PREREQUISITES\n")
        return BOOTSTRAP_SCRIPT.replace('for attempt in $(seq 1 60); do\n',
            setup + 'for attempt in $(seq 1 60); do\n').replace(
                '    loginctl enable-linger mobkit\n',
                KNOWLEDGE_USER_SETUP + '    loginctl enable-linger mobkit\n'
                + KNOWLEDGE_MANAGER_SETUP)
    if not node and not uv:
        return BOOTSTRAP_SCRIPT
    setup = "python3 -I - <<'MOBKIT_PREREQUISITES'\n" + RUNTIME_SETUP + f"\ninstall({node!r}, {uv!r})\nMOBKIT_PREREQUISITES\n"
    return BOOTSTRAP_SCRIPT.replace('for attempt in $(seq 1 60); do\n', setup + 'for attempt in $(seq 1 60); do\n')


def render_manual_prerequisites(deploy, definition):
    """Prepare granted tools in exported GCP scripts using the SSH login user.

    Manual setup reconnects before starting the application and offers lingering
    separately. It must not inherit the guided adapter's fixed user or restart
    an existing user service manager.
    """
    import base64
    import shlex
    import zlib
    from .remote_destination import connection_prerequisites
    commands = set(connection_prerequisites(definition)['mcp_prerequisites'])
    node = bool(commands.intersection({'node', 'npm', 'npx'}))
    uv = bool(commands.intersection({'uv', 'uvx'}))
    local_knowledge = (definition.get('knowledge') or {}).get('mode', 'none') == 'local'
    if not (node or uv or local_knowledge):
        return deploy
    setup = 'set -euo pipefail\nexport DEBIAN_FRONTEND=noninteractive\n'
    changes = []
    if local_knowledge:
        setup += ('installation_user=$1\ninstallation_uid=$2\n'
            '[[ "$installation_uid" =~ ^[0-9]+$ && "$installation_uid" != 0 ]]\n'
            '[[ "$(id -u -- "$installation_user")" == "$installation_uid" ]]\n')
        setup += KNOWLEDGE_SETUP + KNOWLEDGE_USER_SETUP.replace(
            'id -nG mobkit', 'id -nG -- "$installation_user"').replace(
            'usermod -aG docker mobkit', 'usermod -aG docker -- "$installation_user"').replace(
            'runuser -u mobkit', 'runuser -u "$installation_user"')
        changes.append('Local knowledge: install Git and Docker Engine, Buildx and Compose '
            'from Docker\'s official Debian repository; start Docker and give the SSH login '
            'account root-equivalent Docker group access. Setup asks separately about '
            'keeping user services running after disconnecting.')
    if node or uv:
        encoded = base64.b64encode(zlib.compress(RUNTIME_SETUP.encode(), 9)).decode('ascii')
        setup += ("python3 -I - <<'MOBKIT_PREREQUISITES'\nimport base64,zlib\n"
            + f"exec(zlib.decompress(base64.b64decode({encoded!r})))\n"
            + f"install({node!r}, {uv!r})\nMOBKIT_PREREQUISITES\n")
        tools = ', '.join(name for selected, name in ((node, 'Node/npm/npx'), (uv, 'uv/uvx')) if selected)
        changes.append(f'Granted connector prerequisites: install checksum-pinned {tools}. '
            'Other MCP commands still require manual preparation.')
    command = 'sudo bash -c ' + shlex.quote(setup)
    if local_knowledge:
        # Expanded by the destination shell before sudo, never by this computer.
        command += ' -- "$(id -un)" "$(id -u)"'
    base = 'sudo apt-get update && sudo apt-get install -y python3-venv ca-certificates dbus-user-session'
    disclosure = "  printf '%s\\n' " + shlex.quote('\n'.join(changes)) + '\n'
    replacements = (
        ('RULE="$INSTANCE-iap-ssh"', 'HOST_PREPARATION=' + shlex.quote(base + ' && ' + command) + '\nRULE="$INSTANCE-iap-ssh"'),
        ('  exit 0\nfi\ncommand -v gcloud', disclosure + '  exit 0\nfi\ncommand -v gcloud'),
        ('  IFS= read -r -p "Type the project ID to deploy: "', disclosure + '  IFS= read -r -p "Type the project ID to deploy: "'),
        (base + ' && $REMOTE_INSTALL', '$HOST_PREPARATION && $REMOTE_INSTALL'),
    )
    for before, after in replacements:
        if deploy.count(before) != 1:
            raise ValueError('The fixed GCP export template differs from its prerequisite adapter.')
        deploy = deploy.replace(before, after, 1)
    return deploy
