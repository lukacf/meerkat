## Changed

For a staged model switch, the compatibility check now covers the session's existing reasoning level when the target model's supported levels are available at decision time. If no source states the supported levels, the switch is denied. A different rule applies when a source exists but cannot be read; see Known limitations.

## Breaking (behavior)

`brain_swap` stages a switch; it does not immediately change the model. Previously the switch could take effect with an incompatible reasoning level and the next input would be refused. Now, when the target is known not to support the held level, the next input runs on the original model with the original level. The switch, not the input, is denied.

## Upgrade

Do not assume a staged switch took effect or resend the input because the switch was denied. To use the target model, change the held reasoning level to one the target supports, or explicitly clear it, then request the switch again.

The two public switch-request machine inputs gain two fields. Update code that constructs or pattern-matches those inputs; leaving the verdict field empty does not deliver a decision. Separately, the kernel’s `TransitionId` gains six variants. Update exhaustive matches on `TransitionId` to cover them. This is a Rust source migration, not a claim about serialized compatibility.

## Known limitations

Only a target that can be built and has a source for supported levels can take the switch path without a compatibility verdict when that source cannot be read at decision time. Taking that path does not establish that the target accepts the held level. This source distinction was reviewed in code; the two denial tests below did not exercise those alternatives.

The two denial cases (WholeBlob and HeadCanonical) reopened files and reconstructed the session in the same process **after** the switch was denied and the level cleared. The routing records matched, and a later input ran on the retained model. These cases did not exercise process restart, crash recovery, or a switch that was carried out.
