## Changed

For a staged model switch, the compatibility check now covers the session's existing reasoning level when the target model's supported levels are available at decision time. If those levels cannot be determined then, the limitation remains; see Known limitations.

## Breaking (behavior)

`brain_swap` stages a switch; it does not immediately change the model. Previously the switch could take effect with an incompatible reasoning level and the next input would be refused. Now, when the target is known not to support the held level, the next input runs on the original model with the original level. The switch, not the input, is denied.

## Upgrade

Do not assume a staged switch took effect or resend the input because the switch was denied. To use the target model, change the held reasoning level to one the target supports, or explicitly clear it, then request the switch again.

Rust callers constructing the affected public machine inputs must supply the two new fields where required and update affected constructions and patterns across six enum variants. This is a Rust source migration, not a claim about serialized compatibility.

## Known limitations

If supported levels cannot be determined at decision time, staging the switch does not establish that the target accepts the held level.

The two denial cases (WholeBlob and HeadCanonical) reopened files and reconstructed the session in the same process **after** the switch was denied and the level cleared. The routing records matched, and a later input ran on the retained model. These cases did not exercise process restart, crash recovery, or a switch that was carried out.
