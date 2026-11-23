#!/usr/bin/env bash

set -euo pipefail

ROOT="${ROOT:-$(cd "$(dirname "$0")/.." && pwd)}"
CARGO="${CARGO:-$ROOT/scripts/repo-cargo}"
PYTHON="$("$(dirname "${BASH_SOURCE[0]}")/require-python" 3.11 "$(basename "${BASH_SOURCE[0]}")")" || exit 1
JOBS="${MEERKAT_RELEASE_PACKAGING_JOBS:-${MEERKAT_PUBLISH_DRY_RUN_JOBS:-4}}"
LANE_PREFIX="${RUST_LANE_ID:-release-packaging}"

RELEASE_CRATES=()
while IFS= read -r crate; do
    RELEASE_CRATES+=("$crate")
done < <("$ROOT/scripts/release-rust-crates.sh")

"$ROOT/scripts/check-rust-release-config.sh"

tmp_dir="$(mktemp -d)"
trap 'rm -rf "$tmp_dir"' EXIT
TARGET_ROOT="${CARGO_TARGET_DIR:-$tmp_dir/targets}"

LOG_DIR="$tmp_dir/logs"
mkdir -p "$LOG_DIR"

# Space readings: the free space of the filesystems this run writes to and the
# sizes of its own directories, printed before the first package build, after
# the last one and before the facade link smoke, so a run that ends for lack
# of space says how much there was and where it went. Observation only:
# nothing is removed, no check reads a reading, and a reading that cannot be
# taken prints "unavailable". Each boundary runs in a subshell whose errors
# are not printed and whose status is discarded (space_reading below), so no
# failure in it, a missing `df` or `du` included, reaches `set -e` or the
# exit status. No reading writes a file: a here-string needs temporary space,
# which a full disk does not have. Sizes come from `du -P`, which follows no
# symbolic link: the link each per-crate package directory holds into the
# checkout is counted as a link and never entered. Only the per-crate
# directories of this run's release crates are measured, never the rest of a
# caller's CARGO_TARGET_DIR. They are this run's by name: under a persistent
# CARGO_TARGET_DIR they also hold what earlier runs left in them, so the
# figure after the last package build is then not this run's alone. Lines
# carry labels and crate names, not paths.
SPACE_TOP_CRATES=5

# "<free KiB> <total KiB> <filesystem id>" of the filesystem that holds $1
# (of its nearest existing ancestor while $1 does not exist), or nothing.
space_fs_reading() {
    local path="$1"
    while [[ ! -e "$path" && "$path" == */* && "$path" != / ]]; do
        path="${path%/*}"
        [[ -n "$path" ]] || path=/
    done
    [[ -e "$path" ]] || path=.
    df -Pk -- "$path" 2>/dev/null | awk '
        NR == 2 && NF >= 6 && $(NF - 2) ~ /^[0-9]+$/ && $(NF - 4) ~ /^[0-9]+$/ {
            print $(NF - 2), $(NF - 4), $1 ":" $NF
        }'
}

# One line per label and path pair after the boundary's name: the free and
# total space of the filesystem that holds the path. A filesystem an earlier
# pair already named is said to be the same one.
space_free_lines() {
    local boundary="$1" label="" reading="" free="" total="" id="" rest="" note="" index=0
    local -a seen_ids=() seen_labels=()
    shift
    while [[ "$#" -ge 2 ]]; do
        label="$1"
        reading="$(space_fs_reading "$2")" || reading=""
        shift 2
        free="${reading%% *}"
        rest="${reading#* }"
        total="${rest%% *}"
        id="${rest#* }"
        if [[ ! "$free" =~ ^[0-9]+$ || ! "$total" =~ ^[0-9]+$ ]]; then
            printf 'space reading [%s]: %s filesystem: unavailable\n' "$boundary" "$label"
            continue
        fi
        note=""
        index=0
        while [[ "$index" -lt "${#seen_ids[@]}" ]]; do
            if [[ "${seen_ids[$index]}" == "$id" ]]; then
                note=" (same filesystem as ${seen_labels[$index]})"
                break
            fi
            index=$((index + 1))
        done
        if [[ -z "$note" ]]; then
            seen_ids+=("$id")
            seen_labels+=("$label")
        fi
        printf 'space reading [%s]: %s filesystem: %d MiB free of %d MiB%s\n' \
            "$boundary" "$label" "$((free / 1024))" "$((total / 1024))" "$note"
    done
}

# `du -skP` of the directories named: "<KiB><TAB><base name>" lines, largest
# first. -P: no symbolic link is followed, on the command line or below it.
space_du() {
    du -skP -- "$@" 2>/dev/null |
        awk -F '\t' 'NF >= 2 && $1 ~ /^[0-9]+$/ {
            name = $2
            sub(/\/+$/, "", name)
            sub(/.*\//, "", name)
            print $1 "\t" name
        }' |
        sort -rn
}

# The size of one directory of this run, under a label.
space_dir_line() {
    local boundary="$1" label="$2" dir="$3" sizes="" size="" note=""
    if [[ -L "$dir" || ! -d "$dir" ]]; then
        printf 'space reading [%s]: %s: no such directory\n' "$boundary" "$label"
        return 0
    fi
    sizes="$(space_du "$dir")" || note=" (du reported an error: the figure may be partial)"
    size="${sizes%%$'\t'*}"
    if [[ ! "$size" =~ ^[0-9]+$ ]]; then
        printf 'space reading [%s]: %s: unavailable\n' "$boundary" "$label"
        return 0
    fi
    printf 'space reading [%s]: %s: %d MiB%s\n' "$boundary" "$label" "$((size / 1024))" "$note"
}

# The per-crate packaging directories of this run's crates (with whatever a
# persistent target already held in them): their total and the
# SPACE_TOP_CRATES largest, by crate name. One `du` over all of them.
space_crate_lines() {
    local boundary="$1" crate="" sizes="" note=""
    local -a dirs=()
    for crate in "${RELEASE_CRATES[@]}"; do
        if [[ -d "$TARGET_ROOT/$crate" && ! -L "$TARGET_ROOT/$crate" ]]; then
            dirs+=("$TARGET_ROOT/$crate")
        fi
    done
    if [[ "${#dirs[@]}" -eq 0 ]]; then
        printf 'space reading [%s]: packaging target: no per-crate directory\n' "$boundary"
        return 0
    fi
    sizes="$(space_du "${dirs[@]}")" || note=" (du reported an error: the figures may be partial)"
    if [[ -z "$sizes" ]]; then
        printf 'space reading [%s]: packaging target: unavailable\n' "$boundary"
        return 0
    fi
    printf '%s\n' "$sizes" | awk -F '\t' -v boundary="$boundary" -v top="$SPACE_TOP_CRATES" -v note="$note" '
        NF == 2 {
            total += $1
            count += 1
            if (count <= top) {
                size[count] = $1
                name[count] = $2
            }
        }
        END {
            printf "space reading [%s]: packaging target, %d per-crate directories of the release crates, whole contents: %d MiB%s\n", boundary, count, total / 1024, note
            shown = count < top ? count : top
            for (i = 1; i <= shown; i++) {
                printf "space reading [%s]: crate %d of %d by size: %s %d MiB\n", boundary, i, shown, name[i], size[i] / 1024
            }
        }'
}

# One boundary: free space, then what the boundary's name selects, then the
# seconds the readings took.
space_boundary() {
    local boundary="$1" started="$SECONDS"
    space_free_lines "$boundary" "packaging target" "$TARGET_ROOT" "temporary directory" "$tmp_dir" "root" /
    case "$boundary" in
        "packaging end")
            space_crate_lines "$boundary"
            ;;
        "before the link smoke")
            space_dir_line "$boundary" "staged archive copies" "$package_target"
            ;;
    esac
    printf 'space reading [%s]: took %d s\n' "$boundary" "$((SECONDS - started))"
}

# The readings of one boundary, guarded: in a subshell (a failure in it, a
# fatal one included, ends only the subshell), without its error output (a
# shell error message names a path), and with a status that is always 0. The
# line that says a boundary could not be read is written from a subshell
# too: this shell writes nothing here, so a reading cannot be the write that
# ends the script on a standard output nobody reads.
space_reading() {
    ( (space_boundary "$1") || echo "space reading [$1]: unavailable") 2>/dev/null || true
}

run_package() {
    crate="$1"
    cfg="$LOG_DIR/$crate.config.toml"
    log_file="$LOG_DIR/$crate.log"
    result_file="$LOG_DIR/$crate.result"
    target_dir="$TARGET_ROOT/$crate"
    lane_id="$LANE_PREFIX-$crate"

    mkdir -p "$target_dir/package"
    # A persistent CARGO_TARGET_DIR keeps the archives of earlier runs. Remove
    # them, so the archive checks below read only what this run packaged.
    rm -f "$target_dir/package/$crate"-[0-9]*.crate
    rm -f "$target_dir/package/meerkat-core"
    ln -s "$ROOT/crates/meerkat-core" "$target_dir/package/meerkat-core"

    printf '  %-34sPACKAGING\n' "$crate"
    if "$ROOT/scripts/generate-patch-config.sh" "$ROOT" "$crate" > "$cfg" &&
        CARGO_TARGET_DIR="$target_dir" RUST_LANE_ID="$lane_id" \
            "$CARGO" package -p "$crate" --locked --allow-dirty --config "$cfg" > "$log_file" 2>&1; then
        printf '%s:ok\n' "$crate" > "$result_file"
    else
        printf '%s:fail\n' "$crate" > "$result_file"
    fi
}

export ROOT
export CARGO
export LOG_DIR
export TARGET_ROOT
export LANE_PREFIX
export -f run_package

space_reading "packaging start"

# $1 expands in the child shell.
# shellcheck disable=SC2016
printf '%s\n' "${RELEASE_CRATES[@]}" |
    xargs -I{} -P "$JOBS" bash -lc 'run_package "$1"' _ "{}"

space_reading "packaging end"

fail=0
for crate in "${RELEASE_CRATES[@]}"; do
    result_file="$LOG_DIR/$crate.result"
    if [[ ! -f "$result_file" ]]; then
        printf '  %-34sMISSING\n' "$crate"
        fail=1
        continue
    fi

    crate_result="$(cat "$result_file")"
    IFS=: read -r crate_name result <<< "$crate_result"
    if [[ "$result" == ok ]]; then
        printf '  %-34sOK\n' "$crate_name"
        continue
    fi

    fail=1
    printf '  %-34sFAIL\n' "$crate_name"
    if [[ -f "$LOG_DIR/$crate.log" ]]; then
        grep -nE "^\\s*error(:|\\[)" "$LOG_DIR/$crate.log" | head -n 20 || cat "$LOG_DIR/$crate.log"
    fi
done

if [[ "$fail" -ne 0 ]]; then
    echo "Some release crates failed to package"
    exit 1
fi

version="$(awk '
  /^\[workspace.package\]/ { in_workspace_package = 1; next }
  /^\[/ { in_workspace_package = 0 }
  in_workspace_package && $1 == "version" {
    gsub(/"/, "", $3)
    print $3
    exit
  }
' "$ROOT/Cargo.toml")"
if [[ -z "$version" ]]; then
    echo "failed to resolve workspace package version" >&2
    exit 1
fi
package_target="$tmp_dir/package-target"
mkdir -p "$package_target/package"
# Each archive ships exactly the license texts its crate's license
# expression names (scripts/crate-license-files.sh; unknown expressions fail).
license_map="$("$(dirname "${BASH_SOURCE[0]}")/crate-license-files.sh" "${RELEASE_CRATES[@]}")"
while IFS=$'\t' read -r crate expression required_files; do
    archive="$TARGET_ROOT/$crate/package/$crate-$version.crate"
    if [[ ! -f "$archive" ]]; then
        echo "missing verified package archive: $archive" >&2
        exit 1
    fi
    cp "$archive" "$package_target/package/"
    if [[ -z "$required_files" ]]; then
        echo "unsupported license expression for $crate: $expression" >&2
        exit 1
    fi
    archive_files="$(tar -tzf "$archive")"
    for license_file in LICENSE-MIT LICENSE-APACHE; do
        packaged=false
        grep -qx "$crate-$version/$license_file" <<< "$archive_files" && packaged=true
        if [[ " $required_files " == *" $license_file "* ]]; then
            if [[ "$packaged" != true ]]; then
                echo "package archive lacks $license_file: $archive" >&2
                exit 1
            fi
        elif [[ "$packaged" == true ]]; then
            echo "package archive ships $license_file, which its license ($expression) does not name: $archive" >&2
            exit 1
        fi
    done
done <<< "$license_map"

# Every packaged requirement on a sibling release crate is exact (`=$version`):
# 0.x patch releases may break API. scripts/check_rust_release_packaging.py
# holds the manifests to the same rule; this reads what the archives ship.
"$PYTHON" "$(dirname "${BASH_SOURCE[0]}")/check_packaged_internal_requirements.py" \
    "$version" "$package_target/package" "${RELEASE_CRATES[@]}"

space_reading "before the link smoke"

echo "Running published-style facade link smoke..."
MEERKAT_PUBLISHED_FACADE_PACKAGE_TARGET="$package_target" \
    "$ROOT/scripts/check-published-facade-link.sh"

echo "All release crates package successfully"
