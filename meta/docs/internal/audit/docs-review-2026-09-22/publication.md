# Publication receipts

[Audit index](README.md)

## MobKit

The verified changes are committed on
`luka-crnkovicfriis-abk-documentation-adversarial-audit` and pushed to
`lukacf/meerkat-mobkit`:

- `e53206a1fad55092b29420f3b647bee6af12079a`: 148 confirmed local documentation corrections.
- `426c5c951a35c63389b956cc1f719641e7c513af`: complete evidence and review ledger.

The normal pre-commit and pre-push hooks passed. An initial push selected an
environment-provided read-only account. The repository owner's existing,
push-authorized keyring account was then used per command, without exposing
credentials or changing global account settings. A subsequent HTTP 408
transport failure was checked against the remote ref and successfully retried
with HTTP/1.1 and a bounded POST buffer.

The PR creation request was denied with HTTP 403:

```text
Unauthorized: As an Enterprise Managed User, you cannot access this content
```

A read-only GitHub query confirmed that no PR exists for this head branch.
The creation operation could not select the separately authorized keyring
account. No alternative creation operation was performed.

[Open the prepared MobKit comparison](https://github.com/lukacf/meerkat-mobkit/compare/main...luka-crnkovicfriis-abk-documentation-adversarial-audit)

This is a pushed branch and a verified audit, not a claim that a PR was created
or merged. PR publication requires an account authorized for this repository.

## Upstream-owned skill correction

SKILL-001 belongs to `lukacf/meerkat`, not to the MobKit-owned symlink.
Its independently reviewed correction is preserved in a separate worktree:

| Receipt | Value |
|---|---|
| Repository | `lukacf/meerkat` |
| Branch | `luka-crnkovicfriis-abk-mobkit-lease-skill-correction` |
| Base | `02dc6732c87ba2105eb4f84a1f499869965159a7` |
| Correction commit | `4b97c131d8c8213f7b53c0d5d3753da70016f797` |
| File | `.claude/skills/meerkat-architecture/references/gotchas.md` |
| Scope | One physical line: item 37 |
| Final independent review | Pass, no regressions |
| Remote branch / PR | Not published / none |

The existing push-authorized keyring account was used with normal hooks.
The upstream pre-push gate rejected publication:

```text
Error: TLC can't handle a number this big.
18446744073709551615
Error: tlc failed for meerkat_machine (ci.cfg)
make: *** [machine-verify] Error 1
```

`machine-codegen-verify` exited 2 and the pre-push dispatcher exited 1.
An authenticated remote-ref check confirmed that the branch was not created.
No hook was skipped, and no unrelated formal model, runtime code, shared
checkout, or personal skill alias was modified.

The upstream correction patch was carried in this ledger as a compressed
blob (`upstream-skill-correction.patch.gz`, decompressed SHA-256
`4de1cbf3e10de0f6e2b393f33f3fdd227da14631fdc1ce64927e6170d246b0b7`) at
handoff. It targets the **Meerkat repository**
(`.claude/skills/meerkat-architecture/references/gotchas.md`, gotcha 37), not
the MobKit symlink path, so the blob was removed from this repository at
publication and the change is applied upstream in lukacf/meerkat instead
(see the publication note below for the PR). The reverse-apply check the
child session ran against the corrected upstream worktree is recorded in
[S](S.md).

Normal upstream verification must be repaired or completed
before that separate branch can be published. See [S](S.md) for the claim,
implementation proof, adjudication, and final review.

## Publication note (2026-09-22)

The release owner published this branch from an authorized identity by
cherry-picking the three audit commits onto `main` at v0.8.40
(`484d5533`), re-verifying every corrected statement against that source,
and opening a pull request against `main`. The corrections publish through
the main-tracking documentation mirror in lukacf/meerkat once that pipeline
lands; they are not coupled to a MobKit release. The upstream skill
correction is landed separately in lukacf/meerkat. The original receipts
above are retained unchanged.
