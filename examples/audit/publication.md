# Publication follow-up: explicit integer-boundary vocabulary

[Audit index](../AUDIT.md)

The first ordinary push of the completed example audit was blocked by the
installed machine-verification hook. This was a pre-existing model-generation
issue, separate from the 104 example/runtime findings: TLC could not parse the
decimal literal `18446744073709551615` in the live-context observation guard.
The hook was not bypassed.

## Exact correction

The owning DSL now spells that same bound as `u64::MAX`. The parser already
distinguishes this expression from an ordinary integer literal. Production Rust
still uses the exact full-width unsigned maximum; the TLA renderer uses its
existing `RustU64Max` constant and unchanged configuration.

- [Owning guard](https://github.com/lukacf/meerkat/blob/cfee8e73f/meerkat-machine-schema/src/catalog/dsl/meerkat_machine.rs#L27098)
- [Native boundary regressions](https://github.com/lukacf/meerkat/blob/cfee8e73f/meerkat-runtime/tests/gpt_live_generated_authority.rs#L756-L897)
- [Existing parser distinction](https://github.com/lukacf/meerkat/blob/185182c73/meerkat-machine-dsl-core/src/parse.rs#L860-L916)
- [Exact native constant emission](https://github.com/lukacf/meerkat/blob/185182c73/meerkat-machine-dsl-core/src/gen_dispatch.rs#L800-L805)
- [Existing symbolic TLA boundary](https://github.com/lukacf/meerkat/blob/185182c73/meerkat-machine-codegen/src/artifacts.rs#L11068-L11078)
- [Existing renderer contract](https://github.com/lukacf/meerkat/blob/185182c73/meerkat-machine-codegen/tests/render_contracts.rs#L77-L98)

Ordinary generation changed exactly three expressions in the machine model and
six in the dependent composition model. Parent verification compared the entire
files against the preceding commit with only that substitution and confirmed
that the CI/deep configurations were byte-identical. There is no new clamp,
generator policy, model bound, invariant removal, or altered production guard.

## Independent verification

The investigator first generated a scratch candidate from the canonical schema:
the unchanged pinned TLC and `ci.cfg` rejected the original but accepted the
candidate. The parent independently inspected parser/native/TLA lowering and
authorized the exact correction before production files changed.

The implemented native generated-authority tests cover counters above the TLC
representative maximum, a fresh observation at `u64::MAX - 1`, refusal at
`u64::MAX`, exact replay at exhaustion, and eight mismatched replay scopes.
Refusals preserve the entire state; successful cases preserve full-width
ordinals. The parent independently reran all **46 tests** and the existing
TLA-render regression.

The unmodified canonical `make machine-verify` lane passed **23 TLC checks**:
15 machines, seven full composition checks, and the existing bounded adaptive
witness. The script's pre-existing structural/drift handling for the two broad
composition sweeps was unchanged. Strict owner Clippy, all six standalone
lifecycle regressions, formatting, and generation drift also passed.

The MeerkatMachine CI model remains shallow: 9,151 generated states, three
distinct states, depth two. This bounded pass is not presented as exhaustive
native counter or standalone-turn verification; the native regressions cover
those obligations.

Evidence is retained in the session's `formal-publication-fix.json`,
`formal-parent-native-review.log`, and `publication-blocker/` artifacts. The
eight separately observed pre-existing runtime-internal manifest omissions were
not changed, because they did not block the required canonical TLC lane.
