### Changed

For a staged model switch, the compatibility check now covers the session's existing reasoning level when the target model's supported levels are available at decision time.

### Breaking

- Behaviour-only (not measured by the gate): `brain_swap` still reports `staged` or `already_staged`, not the outcome. Previously the switch could take effect with an incompatible level and the next input would be refused. Now, before the next input, a target route that refuses the held level, alone or in combination with the session's other reasoning settings, is denied with `CapabilityPolicy`. The session keeps its model and level and runs the input there. The agent gets no later turn notice of the denial; the routing record and a host log warning identify it.
- Rust API: both `MeerkatMachineInput::RequestFiniteSwitchTurn` and `MeerkatMachineInput::RequestUntilChangedSwitchTurn` gain `target_reasoning_effort` and `inspected_current_identity`. This applies to the inputs in `meerkat_machine_schema::catalog::dsl::meerkat_machine` and `meerkat_runtime::meerkat_machine::dsl`. In `meerkat-machine-kernels`, both `generated::meerkat::inputs::RequestFiniteSwitchTurn` and `generated::meerkat::inputs::RequestUntilChangedSwitchTurn` likewise gain `target_reasoning_effort: Option<TargetReasoningEffortVerdict>` and `inspected_current_identity: Option<SessionLlmIdentity>`.
- Rust API: `meerkat-machine-kernels` adds six variants to `generated::meerkat::TransitionId`:
  - `RequestFiniteSwitchTurnReasoningEffortRefusedIdle`
  - `RequestFiniteSwitchTurnReasoningEffortRefusedAttached`
  - `RequestFiniteSwitchTurnReasoningEffortRefusedRunning`
  - `RequestUntilChangedSwitchTurnReasoningEffortRefusedIdle`
  - `RequestUntilChangedSwitchTurnReasoningEffortRefusedAttached`
  - `RequestUntilChangedSwitchTurnReasoningEffortRefusedRunning`

### Upgrade

After a denial, do not resend the input: it ran on the retained model. Changing or clearing the level does not revive the denied switch. To use the target, choose a supported level or explicitly clear the held level, then request the switch again.

Rust callers must supply both fields when constructing either switch-request input, update named patterns that omit `..`, and handle the six new variants in exhaustive `TransitionId` matches. An empty verdict field does not deliver a compatibility decision. A custom `SessionLlmReconfigureHost` that wants target identity inspection must implement `inspect_target_session_llm_identity`; otherwise the host remains responsible for its judge's acceptance.

### Known limitations

On the stock host, a target whose client cannot be built is held. For a buildable target, **no source for supported levels** means the switch is denied. **An existing source that is temporarily unreadable** is different: the switch can take effect without a compatibility verdict. Requests on the target are then refused as retryable until the levels can be read. If the target does not support the held level, requests remain refused until the level or model changes.
