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
  - anchors: `work_attention_lifecycle`
  - scenarios: `work_attention_pause_resume_stop`
- `ClassifyEligibilityPausedElapsed`
  - anchors: `work_attention_lifecycle`
  - scenarios: `work_attention_pause_resume_stop`
- `ClassifyEligibilityPausedPending`
  - anchors: `work_attention_lifecycle`
  - scenarios: `work_attention_pause_resume_stop`
- `ClassifyEligibilitySuperseded`
  - anchors: `work_attention_lifecycle`
  - scenarios: `work_attention_pause_resume_stop`
- `ClassifyEligibilityStopped`
  - anchors: `work_attention_lifecycle`
  - scenarios: `work_attention_pause_resume_stop`
- `ClassifyAuthorityActive`
  - anchors: `work_attention_lifecycle`
  - scenarios: `work_attention_pause_resume_stop`
- `ClassifyAuthorityPaused`
  - anchors: `work_attention_lifecycle`
  - scenarios: `work_attention_pause_resume_stop`
- `ClassifyAuthoritySuperseded`
  - anchors: `work_attention_lifecycle`
  - scenarios: `work_attention_pause_resume_stop`
- `ClassifyAuthorityStopped`
  - anchors: `work_attention_lifecycle`
  - scenarios: `work_attention_pause_resume_stop`

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
  - anchors: `work_attention_lifecycle`
  - scenarios: `work_attention_pause_resume_stop`
- `AttentionAuthorityClassified`
  - anchors: `work_attention_lifecycle`
  - scenarios: `work_attention_pause_resume_stop`

### Invariants
- `live_has_no_terminal_time`
  - anchors: `work_attention_lifecycle`
  - scenarios: `work_attention_pause_resume_stop`
- `paused_has_pause_state`
  - anchors: `work_attention_lifecycle`
  - scenarios: `work_attention_pause_resume_stop`
- `superseded_records_successor`
  - anchors: `work_attention_lifecycle`
  - scenarios: `work_attention_pause_resume_stop`


<!-- GENERATED_COVERAGE_END -->
