# Archived review evidence

Candidate documents, review records and their manifests retain their original
bytes. The historical `candidate-r5-to-r6.patch` is stored as
[`candidate-r5-to-r6.patch.b64`](candidate-r5-to-r6.patch.b64), so ordinary
text hygiene checks cannot change significant unified-diff context whitespace.
This is a transport encoding, not a revised patch.

The base64 transport file is 9,873 bytes with SHA-256:

```text
25a4a7c06032ef2327240ab5bacacbe96211c32b8776cf4b6d4fa840466f8765
```

This transport digest identifies the packaged text. The decoded original patch
is 7,308 bytes with SHA-256:

```text
8c8a3c8b3303e1cb07471dab2d45dfec937c8597b1ed590dbff2bcbe763d5ae6
```

To verify and recover it from the repository root:

```bash
python3 - <<'PYTHON'
import base64
import hashlib
from pathlib import Path

archive = Path("docs/internal/design/reviews/adr-001")
encoded = (archive / "candidate-r5-to-r6.patch.b64").read_bytes()
assert hashlib.sha256(encoded).hexdigest() == (
    "25a4a7c06032ef2327240ab5bacacbe96211c32b8776cf4b6d4fa840466f8765"
)
patch = base64.b64decode(b"".join(encoded.splitlines()), validate=True)
assert len(patch) == 7308
assert hashlib.sha256(patch).hexdigest() == (
    "8c8a3c8b3303e1cb07471dab2d45dfec937c8597b1ed590dbff2bcbe763d5ae6"
)
Path("/tmp/adr-001-candidate-r5-to-r6.patch").write_bytes(patch)
PYTHON
```

Historical references to the original patch name describe those decoded bytes.
This packaging change does not change any candidate hash or review verdict.
