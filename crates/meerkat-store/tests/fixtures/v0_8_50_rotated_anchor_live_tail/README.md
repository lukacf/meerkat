Written by meerkat v0.8.50 (acb56f784): 33 compaction rewrites through the
prepared HeadCanonical path, each with a live-tail message in the rewrite
mutation, exported (`VACUUM INTO`) right after the first rewrite following the
rewrite-32 row-lineage anchor rotation. v0.8.50 itself cannot cold-load this
session (`Corrupted`); `expected.json` is the transcript it wrote. Used by
`sqlite_store::tests::v0_8_50_rotated_anchor_fixture_loads_through_the_repair`.
