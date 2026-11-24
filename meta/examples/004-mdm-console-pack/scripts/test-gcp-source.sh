#!/usr/bin/env bash
# Local only. Needs Git, Python 3, the pinned Rust toolchain, and a populated
# Cargo registry cache for the locked tree check. Default mode does not build; no mode contacts GCP.
set -euo pipefail
mode=${1:-}
if [[ $# -gt 1 || ( -n "$mode" && "$mode" != --build ) ]]; then
  echo 'usage: test-gcp-source.sh [--build]' >&2; exit 2
fi
if [[ "$mode" == --build ]]; then
  : "${MDM_GCP_CARGO_TARGET_DIR:?set a dedicated warm target directory for --build}"
  [[ "$MDM_GCP_CARGO_TARGET_DIR" = /* && -d "$MDM_GCP_CARGO_TARGET_DIR" ]] || exit 2
fi
here=$(cd "$(dirname "$0")" && pwd)
scratch=$(mktemp -d)
trap 'rm -rf "$scratch"' EXIT
mkdir "$scratch/workspace" "$scratch/bin"
"$here/gcp-source-archive.sh" | tar -xmz -C "$scratch/workspace"
cd "$scratch/workspace"
if git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  echo 'Set TMPDIR outside any Git worktree' >&2; exit 1
fi
cargo metadata --locked --offline --no-deps --format-version 1 > "$scratch/workspace.json"
cargo tree --locked --offline -p meerkat-meta --target "$(rustc -vV | sed -n 's/^host: //p')" >/dev/null
python3 - "$scratch/workspace.json" <<'PY'
import json, sys
from pathlib import Path
root = Path.cwd()
meta = json.loads(Path(sys.argv[1]).read_text())
assert Path(meta['workspace_root']) == root
pack = 'meta/examples/004-mdm-console-pack'
crate, = (p for p in meta['packages'] if p['name'] == 'meerkat-meta')
example, = (t for t in crate['targets'] if t['name'] == 'mdm_mob_target')
assert example['kind'] == ['example']
assert Path(example['src_path']).resolve(strict=True) == root / pack / 'target.rs'
assert (root / 'Cargo.lock').is_file()
assert (root / 'crates/meerkat-core/tests/fixtures/hooks/run_override.json').is_file()
for entry in root.rglob('*'):
    if entry.is_symlink():
        entry.resolve(strict=True).relative_to(root)
    assert entry.name not in {'.git', '.state', 'target', 'node_modules'}
    assert not entry.name.startswith('.env') and entry.suffix not in {'.pem', '.key'}
PY
# Check the worker-facing command and path without compiling: the stand-in
# creates only the executable placeholder Cargo would produce for this argv.
cat > "$scratch/bin/cargo" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
host=$(rustc -vV | sed -n 's/^host: //p')
expected=(build --locked --target "$host" --target-dir "$MDM_GCP_CARGO_TARGET_DIR" -p meerkat-meta --example mdm_mob_target)
[[ $# == ${#expected[@]} ]]
for arg in "${expected[@]}"; do [[ "$1" == "$arg" ]]; shift; done
p="$MDM_GCP_CARGO_TARGET_DIR/$host/debug/examples/mdm_mob_target"
mkdir -p "$(dirname "$p")"; : > "$p"; chmod +x "$p"
SH
chmod +x "$scratch/bin/cargo"
binary=$(MDM_GCP_CARGO_TARGET_DIR="$scratch/target" PATH="$scratch/bin:$PATH" ./meta/examples/004-mdm-console-pack/scripts/build-gcp-target.sh)
[[ -x "$binary" && "$binary" == "$scratch/target/$(rustc -vV | sed -n 's/^host: //p')/debug/examples/mdm_mob_target" ]]
echo 'Verified the extracted locked workspace, target source, and stubbed binary path (no compilation).'

if [[ "$mode" == --build ]]; then
  # The private PATH above was limited to the stubbed invocation.
  binary=$(CARGO_NET_OFFLINE=true ./meta/examples/004-mdm-console-pack/scripts/build-gcp-target.sh)
  printf 'Compiled and linked the locked example offline: %s\n' "$binary"
fi
