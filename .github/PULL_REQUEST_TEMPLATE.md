## Summary

Dogma cleanup PR: no


## Validation


## Review Readiness Packet

Current head SHA:

Command output:

```text

```

## Complexity Delta

- Docs/scripts/tests:
- Non-test implementation:
- Generated/schema churn:

## Old Path Amputation Proof

Required for dogma cleanup PRs. Run:

```bash
make dogma-cleanup-gate DOGMA_PACKET=<packet>
```

The required `Dogma cleanup review gate` CI check runs this gate against the PR
body when the PR title, labels, body marker, or stable changed-file/diff signal
indicates dogma cleanup work, including dogma-shaped ordinary source changes and
structural source deletions in known campaign surfaces, and gate-owning file
changes that could disable this check. In GitHub CI the checker is invoked
directly from the base branch gate source when available, so PR-head Makefile
rewrites cannot skip the gate before the detector runs.

| Old path | Classification | Search literal | Mechanical proof | Follow-up |
| --- | --- | --- | --- | --- |
|  | `deleted` or `made uncallable at boundary` |  |  | none |

## Sample Passing Old Path Amputation Proof

| Old path | Classification | Search literal | Mechanical proof | Follow-up |
| --- | --- | --- | --- | --- |
| `old/session-route` | deleted | `old/session-route` | No production references remain. | none |

## Sample Failing Old Path Amputation Proof

| Old path | Classification | Search literal | Mechanical proof | Follow-up |
| --- | --- | --- | --- | --- |
| `old/session-route` | wrapped | `old/session-route` | A preferred path exists beside the old route. | none |
| `surface_request_lifecycle_helper` | made uncallable at boundary | `surface_request_lifecycle_helper` | Boundary test exercises the replacement wrapper while the old helper remains callable for compatibility. | none |
| `wasm_contract_string_mirror` | retained with linked follow-up | `wasm_contract_string_mirror` | Retained for an active SDK cutover. | LUC-999 |

## Reviewer Dogma Gate

For dogma cleanup PRs, answer before normal bug review: What exact old path
became impossible after this PR?

Move to Rework unless the PR deletes the old path or makes it uncallable at the
boundary. If a dogma cleanup PR reaches review without this gate running, move
it to Rework; the gate is expected to run from explicit PR signal, stable
dogma-shaped diff signal, and gate-owning file changes. Gate-owning files cannot
skip the gate. Wrapper/adapter/mirror proof text, retained rows, or proof that
says the old path remains supported/exported/available/routed/working/callable
for compatibility, or usable by old callers during migration, is Rework.
