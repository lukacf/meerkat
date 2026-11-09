"""Execute generated GCP scripts against a fake CLI without network access."""
from __future__ import annotations

import json
import os
from pathlib import Path
import select
import subprocess
import sys
import tarfile

import pytest

from launchkit.generator import generate_files


PROJECT = "test-agent-project"
INSTANCE = "test-agent-team"
ZONE = "europe-west1-b"
PORT = 4231

FAKE_GCLOUD = '''\
import json
import os
from pathlib import Path
import shutil
import sys

root = Path(os.environ["FAKE_GCLOUD_ROOT"])
args = sys.argv[1:]
with (root / "calls.jsonl").open("a") as stream:
    stream.write(json.dumps(args) + "\\n")
settings = json.loads((root / "settings.json").read_text())
command = args[:3]
if command == ["compute", "instances", "describe"]:
    if settings.get('describe_fails'): sys.exit(2)
    if any(arg.startswith('--format=') for arg in args):
        print(settings.get('ownership_override', (root / 'owner').read_text() if (root / 'owner').exists() else ''))
    sys.exit(0 if settings.get("instance_exists") or (root / 'owner').exists() else 1)
if command == ["compute", "instances", "list"]:
    if settings.get('list_fails'): sys.exit(2)
    if settings.get('instance_exists') or (root / 'owner').exists(): print('test-agent-team')
    sys.exit(0)
if command == ["compute", "firewall-rules", "describe"]:
    if settings.get('firewall_exists') and '--format=json' in args:
        receipt = json.loads((root / 'generated team/.mobkit/gcp-deployment.json').read_text())
        record = {'description': 'MobKit Launch deployment ' + (receipt['id'] if settings.get('firewall_valid') else 'foreign'),
                  'direction': 'INGRESS', 'targetTags': ['test-agent-team-iap'], 'sourceRanges': ['35.235.240.0/20'],
                  'allowed': [{'IPProtocol': 'tcp', 'ports': ['22']}],
                  'network': 'https://www.googleapis.com/compute/v1/projects/test-agent-project/global/networks/default'}
        record.update(settings.get('firewall_overrides', {}))
        print(json.dumps(record))
    sys.exit(0 if settings.get("firewall_exists") else 1)
if command == ["compute", "instances", "create"]:
    if not settings.get('create_fails'):
        (root / 'owner').write_text(args[args.index('--labels') + 1].split('=', 1)[1])
    sys.exit(1 if settings.get("create_fails") else 0)
if args[:2] == ["compute", "scp"]:
    if settings.get('upload_fails'): sys.exit(12)
    shutil.copyfile(args[2], root / "uploaded.tar.gz")
    sys.exit(0)
if args[:2] == ["compute", "ssh"]:
    if settings.get('install_fails') and args[args.index('--command') + 1] != 'true': sys.exit(13)
    sys.exit(0)
if args[:2] == ["services", "enable"]:
    sys.exit(0)
if command == ["compute", "firewall-rules", "create"]:
    sys.exit(0)
raise SystemExit("Unexpected fake gcloud invocation: " + repr(args))
'''


class CloudHarness:
    def __init__(self, root: Path):
        self.root = root
        self.package = root / "generated team"
        self.package.mkdir()
        self.spec = {
            "project": {
                "name": "Test agent team", "slug": INSTANCE,
                "description": "Find useful context and help coordinate the work.",
            },
            "agents": [{
                "id": "lead", "name": "Coordinator", "role": "Coordinate work",
                "instructions": "Use connected information to give clear, grounded answers.",
            }],
            "connections": ["workspace", "github", "telegram"],
            "provider": "openai", "model": "gpt-5.5", "target": "gcp",
            "gcp": {"project": PROJECT, "zone": ZONE},
            "advanced": {"port": PORT},
        }
        self.generate()
        fake_bin = root / "fake-bin"
        fake_bin.mkdir()
        executable = fake_bin / "gcloud"
        executable.write_text(f"#!{sys.executable}\n" + FAKE_GCLOUD)
        executable.chmod(0o755)
        self.env = {
            **os.environ, "PATH": str(fake_bin) + os.pathsep + os.environ["PATH"],
            "FAKE_GCLOUD_ROOT": str(root), "COPYFILE_DISABLE": "1",
        }
        self.configure()

    def generate(self):
        self.files = generate_files(self.spec)
        for name, content in self.files.items():
            destination = self.package / name
            destination.parent.mkdir(parents=True, exist_ok=True)
            destination.write_text(content)

    def configure(self, **settings):
        (self.root / "settings.json").write_text(json.dumps(settings))

    def run(self, response=PROJECT + "\n", script="deploy-gcp.sh", args=()):
        return subprocess.run(
            ["bash", str(self.package / "deploy" / script), *args],
            input=response, env=self.env, text=True, capture_output=True, timeout=10,
        )

    @property
    def calls(self):
        path = self.root / "calls.jsonl"
        return [json.loads(line) for line in path.read_text().splitlines()] if path.exists() else []


@pytest.fixture
def cloud(tmp_path):
    return CloudHarness(tmp_path)


@pytest.mark.parametrize("answer", ["", "\n", "wrong-project\n", PROJECT.upper() + "\n", PROJECT + "-extra\n", " " + PROJECT + "\n", PROJECT + " \n"])
def test_exact_project_confirmation_is_required_before_any_cloud_call(cloud, answer):
    result = cloud.run(response=answer)
    assert result.returncode != 0
    assert cloud.calls == []
    assert not (cloud.root / "uploaded.tar.gz").exists()


def test_script_waits_for_confirmation_before_any_cloud_call(cloud):
    process = subprocess.Popen(
        ["bash", str(cloud.package / "deploy/deploy-gcp.sh")], env=cloud.env,
        stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
    )
    try:
        assert select.select([process.stdout], [], [], 5)[0], "Expected confirmation notice"
        assert "create a billed VM" in process.stdout.readline()
        assert process.poll() is None
        assert cloud.calls == []
        _, error = process.communicate(PROJECT + "\n", timeout=10)
        assert process.returncode == 0, error
        assert cloud.calls[0] == ["compute", "instances", "describe", INSTANCE, "--project", PROJECT, "--zone", ZONE]
    finally:
        if process.poll() is None:
            process.kill()
            process.communicate()


def test_existing_vm_prevents_mutation_and_upload(cloud):
    cloud.configure(instance_exists=True)
    result = cloud.run()
    assert result.returncode != 0
    assert "will not overwrite" in result.stderr
    assert cloud.calls == [["compute", "instances", "describe", INSTANCE, "--project", PROJECT, "--zone", ZONE]]
    assert not (cloud.root / "uploaded.tar.gz").exists()


@pytest.mark.parametrize('failure', ['upload_fails', 'install_fails'])
def test_partial_install_can_resume_only_its_original_vm(cloud, failure):
    cloud.configure(**{failure: True})
    result = cloud.run()
    assert result.returncode != 0 and '--resume' in result.stderr
    assert 'instances delete' in result.stderr
    cloud.configure()
    result = cloud.run(args=('--resume',))
    assert result.returncode == 0, result.stderr
    assert 'verified existing VM' in result.stdout
    assert len([call for call in cloud.calls if call[:3] == ['compute', 'instances', 'create']]) == 1


def test_resume_refuses_vm_with_unrelated_ownership_label(cloud):
    cloud.configure(install_fails=True)
    assert cloud.run().returncode != 0
    before = len(cloud.calls)
    cloud.configure(ownership_override='unrelated')
    result = cloud.run(args=('--resume',))
    assert result.returncode != 0 and 'does not belong' in result.stderr
    assert not any(call[:2] in (['compute', 'scp'], ['compute', 'ssh']) for call in cloud.calls[before:])


def test_happy_path_uploads_only_deployment_files_and_uses_private_access(cloud):
    private_paths = [
        ".env", "data/notes.sqlite3", "workspace/family.txt", ".mobkit/identities.db",
        ".venv/lib/private.py", ".bin/rpc_gateway", ".git/config", "__pycache__/app.pyc",
        "config/.env", "deploy/.env", "config/data/private.txt", "deploy/workspace/private.txt",
    ]
    for name in private_paths:
        destination = cloud.package / name
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_text("private-canary-do-not-upload")

    result = cloud.run()
    assert result.returncode == 0, result.stderr
    assert "VM installed. Configure credentials on the VM" in result.stdout
    calls = cloud.calls
    assert all(call[call.index("--project") + 1] == PROJECT for call in calls)
    vm = next(call for call in calls if call[:3] == ["compute", "instances", "create"])
    assert vm[3] == INSTANCE
    assert vm[vm.index("--zone") + 1] == ZONE
    assert vm[vm.index("--machine-type") + 1] == "e2-medium"
    assert "--no-service-account" in vm and "--no-scopes" in vm
    assert "--shielded-secure-boot" in vm
    firewall = next(call for call in calls if call[:3] == ["compute", "firewall-rules", "create"])
    assert firewall[firewall.index("--rules") + 1] == "tcp:22"
    assert firewall[firewall.index("--source-ranges") + 1] == "35.235.240.0/20"
    assert firewall[firewall.index("--target-tags") + 1] == INSTANCE + "-iap"
    remote_calls = [call for call in calls if call[:2] in (["compute", "ssh"], ["compute", "scp"])]
    assert len(remote_calls) == 3
    assert all("--tunnel-through-iap" in call for call in remote_calls)
    assert all(call[call.index("--zone") + 1] == ZONE for call in remote_calls)
    upload = next(call for call in remote_calls if call[:2] == ["compute", "scp"])
    assert upload[3] == INSTANCE + ":mobkit-package.tar.gz"
    assert not Path(upload[2]).exists(), "Local upload staging archive should be removed"
    install = remote_calls[-1][remote_calls[-1].index("--command") + 1]
    assert "bash install.sh --non-interactive" in install
    assert "setup.py" not in install, "Credentials are entered later in the user's terminal"

    with tarfile.open(cloud.root / "uploaded.tar.gz") as archive:
        members = {member.name.removeprefix("./"): member for member in archive.getmembers()}
        assert {"app.py", "connectors.py", "telegram_bridge.py", "setup.py", ".env.example", "config/mob.toml", "deploy/install-service.sh"} <= members.keys()
        for name, member in members.items():
            assert not Path(name).is_absolute() and ".." not in Path(name).parts
            assert not set(Path(name).parts) & {".env", "data", "workspace", ".mobkit", ".venv", ".bin", ".git", "__pycache__"}
            if member.isfile():
                assert b"private-canary-do-not-upload" not in archive.extractfile(member).read()
    assert 'http_listen(f"127.0.0.1:{0 if preparing else port}")' in cloud.files["app.py"]


def test_unowned_firewall_collision_prevents_vm_creation_and_cleanup_of_foreign_rule(cloud):
    cloud.configure(firewall_exists=True)
    result = cloud.run()
    assert result.returncode != 0 and 'different ownership' in result.stderr
    assert not any(call[:3] == ["compute", "firewall-rules", "create"] for call in cloud.calls)
    assert not any(call[:3] == ["compute", "instances", "create"] for call in cloud.calls)
    assert 'firewall-rules delete' not in result.stderr
    assert 'instances delete' not in result.stderr


def test_exact_owned_scoped_firewall_can_resume_without_recreation(cloud):
    cloud.configure(firewall_exists=True, firewall_valid=True)
    result = cloud.run()
    assert result.returncode == 0, result.stderr
    assert not any(call[:3] == ['compute', 'firewall-rules', 'create'] for call in cloud.calls)
    assert any(call[:3] == ['compute', 'instances', 'create'] for call in cloud.calls)


@pytest.mark.parametrize('override', [{'sourceRanges': ['0.0.0.0/0']}, {'targetTags': []},
                                    {'allowed': [{'IPProtocol': 'tcp', 'ports': ['22', '4231']}]},
                                    {'network': 'projects/foreign/global/networks/default'}, {'disabled': True}])
def test_even_matching_firewall_identity_requires_exact_private_access_scope(cloud, override):
    cloud.configure(firewall_exists=True, firewall_valid=True, firewall_overrides=override)
    result = cloud.run()
    assert result.returncode != 0 and 'different ownership or access scope' in result.stderr
    assert not any(call[:3] == ['compute', 'instances', 'create'] for call in cloud.calls)


def test_failed_vm_creation_prevents_upload_and_remote_install(cloud):
    cloud.configure(create_fails=True)
    result = cloud.run()
    assert result.returncode != 0
    assert not any(call[:2] in (["compute", "ssh"], ["compute", "scp"]) for call in cloud.calls)
    assert not (cloud.root / "uploaded.tar.gz").exists()


def test_tunnel_forwards_only_to_loopback_console_through_iap(cloud):
    result = cloud.run(script="tunnel-gcp.sh")
    assert result.returncode == 0, result.stderr
    assert cloud.calls == [[
        "compute", "ssh", INSTANCE, "--project", PROJECT, "--zone", ZONE,
        "--tunnel-through-iap", "--", "-N", "-L", f"{PORT}:127.0.0.1:{PORT}",
    ]]


def test_dry_run_does_not_invoke_cloud_cli(cloud):
    result = cloud.run(args=("--dry-run",))
    assert result.returncode == 0, result.stderr
    assert "No cloud resources have been changed" in result.stdout
    assert cloud.calls == []


def test_local_package_cloud_command_explains_how_to_rebuild(cloud):
    cloud.spec.update(target="mac", gcp={})
    cloud.generate()
    result = cloud.run()
    assert result.returncode != 0
    assert "Choose Google Cloud in the deployment studio" in result.stderr
    assert "enter your project ID, and rebuild the package" in result.stderr
    assert cloud.calls == []


def test_receipt_survives_ambiguous_lookup_and_remains_resumable(cloud):
    cloud.configure(install_fails=True)
    assert cloud.run().returncode != 0
    receipt = cloud.package / '.mobkit/gcp-deployment.json'
    original = receipt.read_bytes()
    cloud.configure(describe_fails=True, create_fails=True)
    assert cloud.run().returncode != 0
    assert receipt.read_bytes() == original
    cloud.configure()
    assert cloud.run(args=('--resume',)).returncode == 0
    assert len([call for call in cloud.calls if call[:3] == ['compute', 'instances', 'create']]) == 1


def test_ambiguous_first_lookup_does_not_create_receipt_or_resources(cloud):
    cloud.configure(describe_fails=True, list_fails=True)
    assert cloud.run().returncode != 0
    assert not (cloud.package / '.mobkit/gcp-deployment.json').exists()
    assert not any(call[:3] == ['compute', 'instances', 'create'] for call in cloud.calls)


def test_resume_after_failed_vm_creation_reuses_recorded_identity(cloud):
    cloud.configure(create_fails=True)
    assert cloud.run().returncode != 0
    receipt = cloud.package / '.mobkit/gcp-deployment.json'
    original = receipt.read_bytes()
    cloud.configure()
    assert cloud.run(args=('--resume',)).returncode == 0
    assert receipt.read_bytes() == original


@pytest.mark.parametrize('action', ['setup', 'status', 'stop', 'restart', 'remove-service'])
def test_owned_remote_actions_never_provision_upload_or_pass_private_values(cloud, action):
    assert cloud.run().returncode == 0
    before = len(cloud.calls)
    result = cloud.run(response='', args=('--' + action,))
    assert result.returncode == 0, result.stderr
    calls = cloud.calls[before:]
    assert [call[:2] for call in calls] == [['compute', 'instances'], ['compute', 'instances'], ['compute', 'ssh']]
    remote = calls[-1]
    command = remote[remote.index('--command') + 1]
    assert command.endswith('remote_operations.py ' + action)
    assert '--tunnel-through-iap' in remote
    assert (remote[-2:] == ['--', '-t']) is (action == 'setup')
    assert 'private' not in command and '.env' not in command


def test_remote_action_missing_or_changed_receipt_never_creates_replacement(cloud):
    result = cloud.run(response='', args=('--status',))
    assert result.returncode != 0 and 'receipt' in result.stderr
    assert cloud.calls == []
    assert cloud.run().returncode == 0
    before = len(cloud.calls)
    cloud.configure(ownership_override='unrelated')
    result = cloud.run(response='', args=('--stop',))
    assert result.returncode != 0 and 'does not belong' in result.stderr
    assert not any(call[:2] == ['compute', 'ssh'] for call in cloud.calls[before:])


def test_modified_source_refuses_gcp_resume_before_cloud_calls(cloud):
    assert cloud.run().returncode == 0
    before = len(cloud.calls)
    (cloud.package / 'app.py').write_text('changed after acceptance')
    result = cloud.run(args=('--resume',))
    assert result.returncode != 0 and 'source changed' in result.stderr
    assert len(cloud.calls) == before
