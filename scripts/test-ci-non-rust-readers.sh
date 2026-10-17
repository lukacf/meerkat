#!/usr/bin/env bash
# Contract test for scripts/ci-non-rust-readers-guard.mjs, the guard that keeps
# scripts/ci-non-rust-readers.json (the Rust readers of non-Rust inputs) in
# step with the sources.
#
# GREEN: the committed inventory matches the scan of this checkout.
# RED:   an inventory with one reader row deleted fails, naming the reader's
#        file and line, also for an in-crate doc the crate compiles in; an
#        inventory with an invented row fails as stale; a row whose named
#        test its features compile out fails, naming the cfg gate. In
#        a scratch copy of the checkout, each from the clean tree: a new,
#        unlisted reader target fails, naming its file and line; so does a
#        reader in a support file a test target reaches only through
#        include!(concat!(env!("CARGO_MANIFEST_DIR"), "...")); a test in a
#        module that an open and a feature-gated module path both reach
#        counts as compiled in (GREEN), and only through the gated path as
#        compiled out; and an
#        include!() or include_str!() the guard cannot resolve is refused.
#        The scratch copy is a throwaway clone; nothing synthetic touches this
#        checkout.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
GUARD="${REPO_ROOT}/scripts/ci-non-rust-readers-guard.mjs"
INVENTORY="${REPO_ROOT}/scripts/ci-non-rust-readers.json"
TEST_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/meerkat-non-rust-readers.XXXXXX")"
trap 'rm -rf "$TEST_ROOT"' EXIT

fail() {
  printf 'test-ci-non-rust-readers: FAIL: %s\n' "$*" >&2
  exit 1
}

expect_red() {
  local label="$1" log="$2" needle="$3"
  shift 3
  local rc=0
  "$@" >"$log" 2>&1 || rc=$?
  [[ "$rc" -eq 1 ]] || { cat "$log" >&2; fail "${label}: guard exited ${rc}, expected 1"; }
  grep -qF -- "$needle" "$log" || { cat "$log" >&2; fail "${label}: output does not name ${needle}"; }
  printf 'RED   %s: %s\n' "$label" "$(grep -F -- "$needle" "$log" | head -1)"
}

# GREEN on this checkout.
node "$GUARD" >"${TEST_ROOT}/green.log" 2>&1 || { cat "${TEST_ROOT}/green.log" >&2; fail "guard is red on the committed inventory"; }
printf 'GREEN %s\n' "$(grep -F 'ci-non-rust-readers: OK' "${TEST_ROOT}/green.log")"

# RED: delete the help_mobpack_contract row for docs/docs.json.
node - "$INVENTORY" "${TEST_ROOT}/deleted.json" <<'NODE'
const fs = require("node:fs");
const [from, to] = process.argv.slice(2);
const doc = JSON.parse(fs.readFileSync(from, "utf8"));
const rows = doc.paths["docs/docs.json"];
const kept = (rows ?? []).filter((row) => !(row.package === "rkat" && row.target === "test:help_mobpack_contract"));
if (!rows || kept.length === rows.length) throw new Error("fixture: docs/docs.json has no rkat test:help_mobpack_contract row");
if (kept.length) doc.paths["docs/docs.json"] = kept;
else delete doc.paths["docs/docs.json"];
fs.writeFileSync(to, JSON.stringify(doc, null, 2));
NODE
expect_red "deleted inventory row" "${TEST_ROOT}/deleted.log" \
  "missing reader: crates/meerkat-cli/tests/help_mobpack_contract.rs:5 reads docs/docs.json" \
  node "$GUARD" --inventory "${TEST_ROOT}/deleted.json"

# RED: delete the row of an in-crate doc its crate compiles in
# (include_str! of a skill body): the planner selects the crate from it.
node - "$INVENTORY" "${TEST_ROOT}/compiled-in.json" <<'NODE'
const fs = require("node:fs");
const [from, to] = process.argv.slice(2);
const doc = JSON.parse(fs.readFileSync(from, "utf8"));
const key = "crates/meerkat-hooks/skills/hook-authoring/SKILL.md";
if (!doc.paths[key]) throw new Error(`fixture: no ${key} row`);
delete doc.paths[key];
fs.writeFileSync(to, JSON.stringify(doc, null, 2));
NODE
expect_red "deleted compiled-in doc row" "${TEST_ROOT}/compiled-in.log" \
  "missing reader: crates/meerkat-hooks/src/lib.rs:27 reads crates/meerkat-hooks/skills/hook-authoring/SKILL.md (meerkat-hooks lib, scope package)" \
  node "$GUARD" --inventory "${TEST_ROOT}/compiled-in.json"

# RED: an invented row no source produces.
node - "$INVENTORY" "${TEST_ROOT}/stale.json" <<'NODE'
const fs = require("node:fs");
const [from, to] = process.argv.slice(2);
const doc = JSON.parse(fs.readFileSync(from, "utf8"));
doc.paths["docs/index.mdx"] = [
  { package: "rkat", target: "test:help_mobpack_contract", scope: "target", sources: ["crates/meerkat-cli/tests/help_mobpack_contract.rs"] },
];
doc.paths = Object.fromEntries(Object.entries(doc.paths).sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0)));
fs.writeFileSync(to, JSON.stringify(doc, null, 2));
NODE
expect_red "stale inventory row" "${TEST_ROOT}/stale.log" \
  'stale inventory entry: "docs/index.mdx" lists rkat test:help_mobpack_contract' \
  node "$GUARD" --inventory "${TEST_ROOT}/stale.json"

# RED: a row whose named test its build compiles out. meerkat-store's
# release-corpus test sits in the sqlite module (cfg(feature = "sqlite"),
# not a default feature); without the feature its row runs no test.
node - "$INVENTORY" "${TEST_ROOT}/compiled-out.json" <<'NODE'
const fs = require("node:fs");
const [from, to] = process.argv.slice(2);
const doc = JSON.parse(fs.readFileSync(from, "utf8"));
const rows = doc.paths["crates/meerkat-runtime/tests/fixtures/v0_8_10_released_realm/corpus/realm/sessions.sqlite3"] ?? [];
const row = rows.find((candidate) => candidate.package === "meerkat-store" && candidate.target === "lib");
if (!row?.features?.includes("sqlite")) throw new Error("fixture: no meerkat-store lib row with the sqlite feature");
delete row.features;
fs.writeFileSync(to, JSON.stringify(doc, null, 2));
NODE
expect_red "named test compiled out by the row's features" "${TEST_ROOT}/compiled-out.log" \
  'names test exact_released_0810_corpus_imports_and_fully_loads_current_session, which its build (default features) compiles out: cfg(all(feature = "sqlite"' \
  node "$GUARD" --inventory "${TEST_ROOT}/compiled-out.json"

# RED: a new reader in a scratch copy. Clone the committed tree, overlay this
# checkout's guard files (so an uncommitted guard is what runs), and add a
# test target that reads docs/docs.json from outside its crate.
SCRATCH="${TEST_ROOT}/scratch"
git clone --quiet --shared --no-checkout "$REPO_ROOT" "$SCRATCH"
git -C "$SCRATCH" checkout --quiet --detach "$(git -C "$REPO_ROOT" rev-parse HEAD)"
for file in scripts/ci-non-rust-readers.json scripts/ci-non-rust-readers.mjs scripts/ci-non-rust-readers-guard.mjs scripts/rust-test-selector.mjs; do
  cp "${REPO_ROOT}/${file}" "${SCRATCH}/${file}"
done
SCRATCH_GUARD="${SCRATCH}/scripts/ci-non-rust-readers-guard.mjs"
# Each control starts from the committed crates/ tree.
reset_scratch_crates() {
  git -C "$SCRATCH" checkout --quiet -- crates
  git -C "$SCRATCH" clean -fdq -- crates
}

SYNTHETIC="crates/meerkat-machine-schema/tests/synthetic_non_rust_reader.rs"
cat >"${SCRATCH}/${SYNTHETIC}" <<'RUST'
#[test]
fn synthetic_reader_of_the_docs_navigation() {
    let navigation = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../docs/docs.json"));
    assert!(navigation.is_ok());
}
RUST
expect_red "new unlisted reader in a scratch copy" "${TEST_ROOT}/synthetic.log" \
  "missing reader: ${SYNTHETIC}:3 reads docs/docs.json (meerkat-machine-schema test:synthetic_non_rust_reader" \
  node "$SCRATCH_GUARD"

# A reader in a support file that is no Cargo target of its own: the
# hooks_contracts test target reaches it only through a manifest-relative
# include!. The walk must follow it and name the reader.
reset_scratch_crates
HOOKS="crates/meerkat-core/tests/hooks_contracts.rs"
H1_SUPPORT="crates/meerkat-core/tests/support/h1_reader.rs"
mkdir -p "${SCRATCH}/crates/meerkat-core/tests/support"
cat >"${SCRATCH}/${H1_SUPPORT}" <<'RUST'
#[test]
fn h1_reader_reads_the_docs_navigation() {
    let navigation = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../docs/docs.json"));
    assert!(navigation.is_ok());
}
RUST
printf '%s\n' 'include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/support/h1_reader.rs"));' >>"${SCRATCH}/${HOOKS}"
expect_red "reader reached through include!(concat!(env!(CARGO_MANIFEST_DIR), ...))" "${TEST_ROOT}/include.log" \
  "missing reader: ${H1_SUPPORT}:3 reads docs/docs.json (meerkat-core test:hooks_contracts" \
  node "$SCRATCH_GUARD"

# A module file two module paths reach with different cfg gates: the modules
# it declares inherit both. meerkat-store's lib gets an open and a
# feature-gated path to one module; the gated one, declared last, is walked
# first. The reader test in that module's child is compiled in through the
# open path (GREEN); with the open path gone it is compiled out (RED).
reset_scratch_crates
STORE_LIB="crates/meerkat-store/src/lib.rs"
PROBE="crates/meerkat-store/src/gates_probe"
mkdir -p "${SCRATCH}/${PROBE}"
printf '%s\n' 'mod inner;' >"${SCRATCH}/${PROBE}/mod.rs"
cat >"${SCRATCH}/${PROBE}/inner.rs" <<'RUST'
#[cfg(test)]
mod tests {
    #[test]
    fn gates_probe_reads_the_docs_navigation() {
        let navigation = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../docs/docs.json"));
        assert!(navigation.is_ok());
    }
}
RUST
node - "${SCRATCH}/scripts/ci-non-rust-readers.json" "${TEST_ROOT}/gates-probe.json" <<'NODE'
const fs = require("node:fs");
const [from, to] = process.argv.slice(2);
const doc = JSON.parse(fs.readFileSync(from, "utf8"));
doc.paths["docs/docs.json"].push({
  package: "meerkat-store",
  target: "lib",
  scope: "target",
  tests: ["gates_probe_reads_the_docs_navigation"],
  sources: ["crates/meerkat-store/src/gates_probe/inner.rs"],
});
fs.writeFileSync(to, JSON.stringify(doc, null, 2));
NODE
cp "${SCRATCH}/${STORE_LIB}" "${TEST_ROOT}/store-lib.rs"
printf '%s\n' '#[path = "gates_probe/mod.rs"]' 'mod gates_probe_open;' '#[cfg(feature = "gates-probe-off")]' '#[path = "gates_probe/mod.rs"]' 'mod gates_probe_gated;' >>"${SCRATCH}/${STORE_LIB}"
node "$SCRATCH_GUARD" --inventory "${TEST_ROOT}/gates-probe.json" >"${TEST_ROOT}/gates-probe.log" 2>&1 \
  || { cat "${TEST_ROOT}/gates-probe.log" >&2; fail "a test reached through an open module path counted as compiled out"; }
printf 'GREEN test reached through an open and a gated module path: %s\n' "$(grep -F 'ci-non-rust-readers: OK' "${TEST_ROOT}/gates-probe.log")"
cp "${TEST_ROOT}/store-lib.rs" "${SCRATCH}/${STORE_LIB}"
printf '%s\n' '#[cfg(feature = "gates-probe-off")]' '#[path = "gates_probe/mod.rs"]' 'mod gates_probe_gated;' >>"${SCRATCH}/${STORE_LIB}"
expect_red "test reached only through a gated module path" "${TEST_ROOT}/gates-probe-red.log" \
  'names test gates_probe_reads_the_docs_navigation, which its build (default features) compiles out: cfg(feature = "gates-probe-off")' \
  node "$SCRATCH_GUARD" --inventory "${TEST_ROOT}/gates-probe.json"

# An include the guard cannot resolve is refused, not skipped: an include!
# of generated source under OUT_DIR, and an include_str! of a path from the
# environment.
reset_scratch_crates
UNRESOLVED_LINE=$(($(wc -l <"${SCRATCH}/${HOOKS}") + 1))
printf '%s\n' \
  'include!(concat!(env!("OUT_DIR"), "/h1_generated.rs"));' \
  'const H1_DATA: &str = include_str!(env!("H1_DATA_PATH"));' >>"${SCRATCH}/${HOOKS}"
expect_red "unresolvable include! refused" "${TEST_ROOT}/unresolved.log" \
  "unresolved include: ${HOOKS}:${UNRESOLVED_LINE} include!(concat!(env!(\"OUT_DIR\"),\"/h1_generated.rs\")) (meerkat-core test:hooks_contracts)" \
  node "$SCRATCH_GUARD"
NEEDLE="unresolved include: ${HOOKS}:$((UNRESOLVED_LINE + 1)) include_str!(env!(\"H1_DATA_PATH\")) (meerkat-core test:hooks_contracts)"
grep -qF -- "$NEEDLE" "${TEST_ROOT}/unresolved.log" || { cat "${TEST_ROOT}/unresolved.log" >&2; fail "unresolvable include_str! is not refused: ${NEEDLE}"; }
printf 'RED   unresolvable include_str! refused: %s\n' "$(grep -F -- "$NEEDLE" "${TEST_ROOT}/unresolved.log" | head -1)"

printf 'test-ci-non-rust-readers: OK\n'
