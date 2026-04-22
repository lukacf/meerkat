# Lane C Checklist

- [x] `C-01` `DOGMA-12` Make classified admission the single semantic owner of untrusted ingress decisions. Done when transport does not independently decide the same untrusted-ingress fact.
- [x] `C-02` `DOGMA-12` Preserve auth-exempt bootstrap/idempotency traffic through the single-owner admission path. Done when auth-exempt bridge traffic is accepted or dropped only by the canonical admission seam.
- [x] `C-03` `DOGMA-12` Move ack emission after final admission outcome. Done when no ack is sent before admission has definitively accepted the ingress item.
- [x] `C-04` `DOGMA-12` Remove trust rereads for the same ingress fact. Done when the trust decision is computed once and reused through classification/admission.
- [x] `C-05` `DOGMA-19` Publish trust only after DSL bind success. Done when bind rejection cannot leave a peer trusted.
- [x] `C-06` `DOGMA-19` Keep old trust active until DSL authorize success and publish new trust only after success. Done when authorize rejection cannot strand the wrong supervisor as trusted.
- [x] `C-07` `DOGMA-19` Add explicit rollback or deferred-publication logic for every trust-mutating failure path. Done when handler-level tests cover reject-after-prepare cases.
- [x] `C-08` `DOGMA-12` `DOGMA-19` Add or update the trust/ack defensive scan. Done when the scan is checked in and wired for lane/final gating.
- [x] `C-09` Self static review. Done when the trust publication order is clearly commit-first and not just renamed comments.
- [x] `C-10` Handoff note. Done when handoff commit, changed files, and risks are recorded in [C-plan.md](/Users/luka/.codex/worktrees/ec4d/meerkat/.rct/round5/agents/C-plan.md).
- [x] `C-11` Generated-output request. Done when any regen need is recorded explicitly, or marked `none`.
