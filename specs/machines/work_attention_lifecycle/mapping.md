# WorkAttentionLifecycleMachine Mapping Note

<!-- GENERATED_COVERAGE_START -->
## Generated Coverage
This section is generated from the Rust machine catalog. Do not edit it by hand.

### Machine
- `WorkAttentionLifecycleMachine`

### Code Anchors
- `work_attention_lifecycle` (machine `WorkAttentionLifecycleMachine`): `meerkat-workgraph/src/machine.rs` — WorkAttentionMachine domain-facing lifecycle transition seam over Pause, Resume, Stop, and Supersede; effects Paused, Resumed, Stopped, Superseded; invariants active_has_no_pause_deadline, paused_has_pause_deadline, stopped_has_stop_time, superseded_has_target; revision, timed pause eligibility, stopped state, and supersession target ownership

### Scenarios
- `work_attention_pause_resume_stop` — PauseActive, PausePaused, ResumePaused, SupersedeActive, SupersedePaused, StopActive, StopPaused, AttentionPaused, AttentionResumed, AttentionSuperseded, AttentionStopped, live_has_no_terminal_time, paused_has_pause_state, superseded_records_successor, timed pause eligibility, CAS revision, and terminal work item attention stop stay under WorkAttentionLifecycleMachine authority

### Transitions
- `PauseActive`
  - anchors: `work_attention_lifecycle`
  - scenarios: `work_attention_pause_resume_stop`
- `PausePaused`
  - anchors: `work_attention_lifecycle`
  - scenarios: `work_attention_pause_resume_stop`
- `ResumePaused`
  - anchors: `work_attention_lifecycle`
  - scenarios: `work_attention_pause_resume_stop`
- `SupersedeActive`
  - anchors: `work_attention_lifecycle`
  - scenarios: `work_attention_pause_resume_stop`
- `SupersedePaused`
  - anchors: `work_attention_lifecycle`
  - scenarios: `work_attention_pause_resume_stop`
- `StopActive`
  - anchors: `work_attention_lifecycle`
  - scenarios: `work_attention_pause_resume_stop`
- `StopPaused`
  - anchors: `work_attention_lifecycle`
  - scenarios: `work_attention_pause_resume_stop`
- `ClassifyEligibilityActive`
  - anchors: (unclaimed)
  - scenarios: (unclaimed)
- `ClassifyEligibilityPausedElapsed`
  - anchors: (unclaimed)
  - scenarios: (unclaimed)
- `ClassifyEligibilityPausedPending`
  - anchors: (unclaimed)
  - scenarios: (unclaimed)
- `ClassifyEligibilitySuperseded`
  - anchors: (unclaimed)
  - scenarios: (unclaimed)
- `ClassifyEligibilityStopped`
  - anchors: (unclaimed)
  - scenarios: (unclaimed)
- `ClassifyAuthorityActive`
  - anchors: (unclaimed)
  - scenarios: (unclaimed)
- `ClassifyAuthorityPaused`
  - anchors: (unclaimed)
  - scenarios: (unclaimed)
- `ClassifyAuthoritySuperseded`
  - anchors: (unclaimed)
  - scenarios: (unclaimed)
- `ClassifyAuthorityStopped`
  - anchors: (unclaimed)
  - scenarios: (unclaimed)

### Effects
- `AttentionPaused`
  - anchors: `work_attention_lifecycle`
  - scenarios: `work_attention_pause_resume_stop`
- `AttentionResumed`
  - anchors: `work_attention_lifecycle`
  - scenarios: `work_attention_pause_resume_stop`
- `AttentionSuperseded`
  - anchors: `work_attention_lifecycle`
  - scenarios: `work_attention_pause_resume_stop`
- `AttentionStopped`
  - anchors: `work_attention_lifecycle`
  - scenarios: `work_attention_pause_resume_stop`
- `AttentionEligibilityClassified`
  - anchors: (unclaimed)
  - scenarios: (unclaimed)
- `AttentionAuthorityClassified`
  - anchors: (unclaimed)
  - scenarios: (unclaimed)

### Invariants
- `live_has_no_terminal_time`
  - anchors: (unclaimed)
  - scenarios: `work_attention_pause_resume_stop`
- `paused_has_pause_state`
  - anchors: `work_attention_lifecycle`
  - scenarios: `work_attention_pause_resume_stop`
- `superseded_records_successor`
  - anchors: (unclaimed)
  - scenarios: `work_attention_pause_resume_stop`


<!-- GENERATED_COVERAGE_END -->
