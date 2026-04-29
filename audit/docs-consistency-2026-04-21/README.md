# Documentation Consistency Audit

This folder contains a structure- and consistency-focused audit of `docs/`,
generated on 2026-04-21.

The focus is not just factual correctness but overall information architecture:

- section boundaries
- nav ordering
- category fit
- conceptual gaps
- duplicated or inconsistent patterns
- dead or weak links
- missing cross-links
- audience mismatch across overview / concepts / guides / examples

Assigned scopes:

1. `agent-01-overview-and-structure.md`
   - `docs/docs.json`
   - `docs/introduction.mdx`
   - `docs/quickstart.mdx`
   - tab/group ordering
   - high-level docs-site structure and missing/odd categories

2. `agent-02-concepts-consistency.md`
   - `docs/concepts/*`
   - conceptual boundaries, missing concepts, overlap, terminology mismatch

3. `agent-03-guides-consistency.md`
   - `docs/guides/*`
   - guide placement, coverage gaps, ordering, duplication, dead/weak links, concept-vs-guide confusion

4. `agent-04-examples-consistency.md`
   - `docs/examples/*`
   - example organization, progression, audience fit, duplication, cross-linking, missing beginner/advanced framing

Each report should use this structure:

- Scope reviewed
- Files reviewed
- Findings
- Suggested follow-ups

After the four reports are complete, a unified checklist will be written in:

- `master-consistency-checklist.md`
