#!/usr/bin/env python3
"""Scaffold Luka Loop files into a repo.
Usage: luka_scaffold.py /path/to/repo [--force]
"""
from pathlib import Path
import shutil
import sys

force = False
args = [a for a in sys.argv[1:] if not a.startswith('-')]
if '--force' in sys.argv:
    force = True

if not args:
    repo = Path.cwd()
else:
    repo = Path(args[0]).expanduser().resolve()

skill_root = Path(__file__).resolve().parents[1]
asset_root = skill_root / 'assets' / 'luka_loop' / '.rct'

if not asset_root.exists():
    print(f"Missing assets: {asset_root}", file=sys.stderr)
    sys.exit(1)

rct_dir = repo / '.rct'

for src in asset_root.rglob('*'):
    if src.is_dir():
        continue
    rel = src.relative_to(asset_root)
    dst = rct_dir / rel
    dst.parent.mkdir(parents=True, exist_ok=True)
    if dst.exists() and not force:
        continue
    shutil.copy2(src, dst)

# Mark scripts executable
for script in (rct_dir / 'scripts').glob('*'):
    script.chmod(0o755)

print(f"Scaffolded Luka Loop into: {rct_dir}")
print("Next steps:")
print("1) Fill .rct/spec.yaml, .rct/plan.yaml, .rct/checklist.yaml")
print("2) Validate checklist: .rct/scripts/validate_checklist.py")
print("3) Render checklist: .rct/scripts/render_checklist.py")
print("4) Run loop: .rct/scripts/luka_loop.sh")
