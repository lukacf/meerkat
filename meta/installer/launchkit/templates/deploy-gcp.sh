#!/usr/bin/env bash
set -euo pipefail
cd -- "$(dirname -- "$0")/.."
PROJECT=__PROJECT__
ZONE=__ZONE__
INSTANCE=__INSTANCE__
MACHINE=__MACHINE__
PORT=__PORT__
RULE="$INSTANCE-iap-ssh"
RESUME=false
OPERATION=""
case "${1:-}" in
  ""|--dry-run) ;;
  --resume) RESUME=true ;;
  --setup|--status|--stop|--restart|--remove-service) RESUME=true; OPERATION="${1#--}" ;;
  *) echo "Use --dry-run, --resume, --setup, --status, --stop, --restart or --remove-service." >&2; exit 1 ;;
esac
if [[ "${1:-}" == "--dry-run" ]]; then
  printf 'Deployment plan\nProject: %s\nZone: %s\nNew VM: %s (%s, Debian 12, 30 GB disk)\nAccess: IAP SSH tunnel to 127.0.0.1:%s\nNo cloud resources have been changed.\n' "$PROJECT" "$ZONE" "$INSTANCE" "$MACHINE" "$PORT"
  exit 0
fi
command -v gcloud >/dev/null || { echo "Install Google Cloud CLI and run gcloud auth login first." >&2; exit 1; }
if [[ -z "$PROJECT" ]]; then echo "Choose Google Cloud in the deployment studio, enter your project ID, and rebuild the package." >&2; exit 1; fi
if [[ -z "$OPERATION" ]]; then
  printf 'This will create a billed VM and a scoped IAP SSH firewall rule in %s unless --resume finds this installation.\n' "$PROJECT"
  IFS= read -r -p "Type the project ID to deploy: " CONFIRMED_PROJECT
  [[ "$CONFIRMED_PROJECT" == "$PROJECT" ]] || { echo "Cancelled."; exit 1; }
fi
command -v python3 >/dev/null || { echo "Python 3 is needed to record this deployment's recovery identity." >&2; exit 1; }
CONFIGURATION="$(python3 remote_operations.py digest)"
RECEIPT=".mobkit/gcp-deployment.json"
VM_OWNED=false
FIREWALL_OWNED=false
print_recovery() {
  printf '\nIf provisioning or installation stops, keep this package folder and run:\n  bash deploy/deploy-gcp.sh --resume\n'
  printf 'Resume only accepts the VM whose deployment label matches this folder. Credentials and runtime data are preserved.\n'
  printf 'Configure and start without editing files: bash deploy/deploy-gcp.sh --setup\n'
  printf 'Later: bash deploy/deploy-gcp.sh --status (or --stop, --restart, --remove-service).\n'
  printf 'Stopping/removing the application service does not delete the billed VM or its data.\n'
  printf 'To inspect it: gcloud compute instances describe %s --project %s --zone %s\n' "$INSTANCE" "$PROJECT" "$ZONE"
  if [[ "$VM_OWNED" == true ]]; then
    printf 'To remove this verified owned VM: gcloud compute instances delete %s --project %s --zone %s\n' "$INSTANCE" "$PROJECT" "$ZONE"
  fi
  if [[ "$FIREWALL_OWNED" == true ]]; then
    printf 'Remove this verified owned firewall rule when no longer needed: gcloud compute firewall-rules delete %s --project %s\n' "$RULE" "$PROJECT"
  fi
}
PACKAGE_ARCHIVE=""
cleanup() {
  result=$?
  [[ -z "$PACKAGE_ARCHIVE" ]] || rm -f "$PACKAGE_ARCHIVE"
  if [[ "$result" != 0 ]]; then print_recovery >&2; fi
}
trap cleanup EXIT
if [[ -f "$RECEIPT" && "$RESUME" != true ]]; then
  echo "This package has a deployment receipt. Keep it intact and use --resume to reconnect to that VM." >&2
  exit 1
fi
if [[ "$RESUME" == true ]]; then
  [[ -f "$RECEIPT" ]] || { echo "No recovery receipt found. Use the original package folder." >&2; exit 1; }
  DEPLOYMENT_ID="$(python3 - "$RECEIPT" "$PROJECT" "$ZONE" "$INSTANCE" "$CONFIGURATION" <<'PY'
import json, pathlib, re, sys
record = json.loads(pathlib.Path(sys.argv[1]).read_text())
expected = dict(zip(('project', 'zone', 'instance'), sys.argv[2:5]))
if any(record.get(key) != value for key, value in expected.items()) or record.get('configuration') != sys.argv[5] or not re.fullmatch(r'[a-f0-9]{32}', record.get('id', '')):
    raise SystemExit('Recovery receipt does not match this project, VM and configuration. Use the original package.')
print(record['id'])
PY
)"
fi
if gcloud compute instances describe "$INSTANCE" --project "$PROJECT" --zone "$ZONE" >/dev/null 2>&1; then
  if [[ "$RESUME" != true || ! -f "$RECEIPT" ]]; then
    echo "A VM with this name already exists. This installer will not overwrite it. Use --resume only with this deployment's original package folder." >&2
    exit 1
  fi
  REMOTE_ID="$(gcloud compute instances describe "$INSTANCE" --project "$PROJECT" --zone "$ZONE" --format='value(labels.mobkit_launch)')"
  [[ "$REMOTE_ID" == "$DEPLOYMENT_ID" ]] || { echo "This VM does not belong to this installation. Resume refused." >&2; exit 1; }
  VM_OWNED=true
  printf 'Resuming installation on the verified existing VM.\n'
else
  [[ -z "$OPERATION" ]] || { echo "The owned VM is unavailable. This operation never creates replacement resources." >&2; exit 1; }
  # A failed describe can mean missing permissions or a transient API outage.
  # A successful scoped list must confirm absence before creating anything.
  if ! FOUND_INSTANCE="$(gcloud compute instances list --project "$PROJECT" --filter="name=$INSTANCE AND zone:($ZONE)" --format='value(name)')"; then
    echo "Could not verify whether the VM exists. No resources or receipt were changed. Check gcloud access and retry." >&2
    exit 1
  fi
  [[ -z "$FOUND_INSTANCE" ]] || { echo "The VM exists but could not be inspected. No resources were changed." >&2; exit 1; }
  umask 077
  mkdir -p .mobkit
  if [[ "$RESUME" != true ]]; then
  DEPLOYMENT_ID="$(python3 - "$RECEIPT" "$PROJECT" "$ZONE" "$INSTANCE" "$CONFIGURATION" <<'PY'
import json, pathlib, sys, uuid
record = dict(zip(('project', 'zone', 'instance'), sys.argv[2:5]))
record.update(id=uuid.uuid4().hex, configuration=sys.argv[5])
pathlib.Path(sys.argv[1]).write_text(json.dumps(record))
print(record['id'])
PY
)"
  fi
  print_recovery
  gcloud services enable compute.googleapis.com iap.googleapis.com --project "$PROJECT"
  if RULE_JSON="$(gcloud compute firewall-rules describe "$RULE" --project "$PROJECT" --format=json 2>/dev/null)"; then
    if ! printf '%s' "$RULE_JSON" | python3 -c '
import json, sys
rule = json.load(sys.stdin)
expected = {"description": "MobKit Launch deployment " + sys.argv[1], "direction": "INGRESS", "targetTags": [sys.argv[2] + "-iap"], "sourceRanges": ["35.235.240.0/20"], "allowed": [{"IPProtocol": "tcp", "ports": ["22"]}]}
network = "/projects/" + sys.argv[3] + "/global/networks/default"
if any(rule.get(key) != value for key, value in expected.items()) or not rule.get("network", "").endswith(network) or rule.get("disabled", False) or any(rule.get(key) for key in ("sourceTags", "sourceServiceAccounts", "targetServiceAccounts", "denied")):
    raise SystemExit("An existing firewall rule has different ownership or access scope. No VM was created and that rule was not changed.")
' "$DEPLOYMENT_ID" "$INSTANCE" "$PROJECT"; then exit 1; fi
    FIREWALL_OWNED=true
  else
    gcloud compute firewall-rules create "$RULE" --project "$PROJECT" --network default \
      --direction INGRESS --action ALLOW --rules tcp:22 --source-ranges 35.235.240.0/20 --target-tags "$INSTANCE-iap" \
      --description "MobKit Launch deployment $DEPLOYMENT_ID"
    FIREWALL_OWNED=true
  fi
  gcloud compute instances create "$INSTANCE" --project "$PROJECT" --zone "$ZONE" \
    --machine-type "$MACHINE" --image-family debian-12 --image-project debian-cloud \
    --boot-disk-size 30GB --boot-disk-type pd-balanced --tags "$INSTANCE-iap" \
    --labels "mobkit_launch=$DEPLOYMENT_ID" --no-service-account --no-scopes --shielded-secure-boot
  VM_OWNED=true
fi
if [[ -n "$OPERATION" ]]; then
  # The receipt and VM ownership label were checked above. No source upload,
  # cloud provisioning or credentials in command arguments are involved.
  SSH_FLAGS=()
  [[ "$OPERATION" != setup ]] || SSH_FLAGS+=(-- -t)
  exec gcloud compute ssh "$INSTANCE" --project "$PROJECT" --zone "$ZONE" --tunnel-through-iap \
    --command "cd \"\$HOME/src/$INSTANCE\" && .venv/bin/python -I remote_operations.py $OPERATION" ${SSH_FLAGS[@]+"${SSH_FLAGS[@]}"}
fi
PACKAGE_ARCHIVE="$(mktemp -t mobkit-package.XXXXXX)"
# Private credentials, local state, caches and workspaces are never uploaded.
# The helper verifies SHA256SUMS and includes all accepted ordinary assets/UI.
python3 remote_operations.py archive "$PACKAGE_ARCHIVE"
for attempt in 1 2 3 4 5 6; do
  if gcloud compute ssh "$INSTANCE" --project "$PROJECT" --zone "$ZONE" --tunnel-through-iap --command true; then break; fi
  if [[ "$attempt" == 6 ]]; then echo "VM created, but SSH is not ready. Follow the recovery instructions in README.md." >&2; exit 1; fi
  sleep 10
done
gcloud compute scp "$PACKAGE_ARCHIVE" "$INSTANCE:mobkit-package.tar.gz" --project "$PROJECT" --zone "$ZONE" --tunnel-through-iap
REMOTE_INSTALL="$(python3 - "$INSTANCE" "$DEPLOYMENT_ID" "$CONFIGURATION" <<'PY'
import hashlib, pathlib, shlex, sys
# Execute only the exact transferred host helper, then let it verify the full
# archive and destination ownership before writing any ordinary source file.
code = '''import gzip, hashlib, io, pathlib, sys, tarfile, zlib
root = pathlib.Path.home() / 'src' / sys.argv[1]
archive = pathlib.Path.home() / 'mobkit-package.tar.gz'
MAX_SOURCE = 64 * 1024 * 1024
MAX_ARCHIVE_BYTES = MAX_SOURCE + 1024 * 1024
MAX_TAR_BYTES = MAX_SOURCE + 16 * 1024 * 1024
with archive.open('rb') as stream:
    data = stream.read(MAX_ARCHIVE_BYTES + 1)
if len(data) > MAX_ARCHIVE_BYTES:
    raise SystemExit('Uploaded installation archive exceeded its compressed size bound.')
try:
    with gzip.GzipFile(fileobj=io.BytesIO(data)) as compressed:
        tar_bytes = compressed.read(MAX_TAR_BYTES + 1)
except (EOFError, OSError, zlib.error):
    raise SystemExit('Uploaded installation archive could not be decompressed.') from None
if len(tar_bytes) > MAX_TAR_BYTES:
    raise SystemExit('Uploaded installation archive exceeded its decompressed size bound.')
with tarfile.open(fileobj=io.BytesIO(tar_bytes), mode='r:') as stream:
    helper = stream.extractfile('remote_operations.py').read(16 * 1024 * 1024 + 1)
if hashlib.sha256(helper).hexdigest() != sys.argv[4]:
    raise SystemExit('Uploaded installation helper differs from accepted source.')
namespace = {'__name__': 'mobkit_source_receiver', '__file__': str(root / 'remote_operations.py')}
exec(compile(helper, 'accepted remote_operations.py', 'exec'), namespace)
namespace['receive_archive'](archive, root, sys.argv[2], sys.argv[3])
'''
arguments = ['python3', '-I', '-c', code, *sys.argv[1:4], hashlib.sha256(pathlib.Path('remote_operations.py').read_bytes()).hexdigest()]
print(' '.join(shlex.quote(value) for value in arguments))
PY
)"
gcloud compute ssh "$INSTANCE" --project "$PROJECT" --zone "$ZONE" --tunnel-through-iap \
  --command "sudo apt-get update && sudo apt-get install -y python3-venv ca-certificates dbus-user-session && $REMOTE_INSTALL && rm mobkit-package.tar.gz && cd \"\$HOME/src/$INSTANCE\" && bash install.sh --non-interactive"
printf '\nVM installed. Configure credentials on the VM:\n'
printf 'Run one guided setup/start operation from this package folder:\n  bash deploy/deploy-gcp.sh --setup\n'
printf 'Private values are entered only in the VM terminal. Start reports actual native bootstrap and Console checks.\n'
printf 'Later: bash deploy/deploy-gcp.sh --status (or --stop, --restart, --remove-service).\n'
printf '\nOpen the console after starting the service:\nbash deploy/tunnel-gcp.sh\n'
print_recovery
