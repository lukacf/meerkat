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
#
# The script's wait for every `df` and `du` is bounded (space_bounded below):
# the measurements of one boundary share SPACE_READING_BOUND seconds. One
# that is still running when they are used up is signalled and printed as
# "timed out", the ones after it are printed as "skipped", and the script
# goes on without waiting for it. Where the bound cannot be kept (a bash
# older than 4.4) no measurement is started and the boundary prints
# "skipped: no bounded measurement available".
#
# What the bound does not say:
# - The signals go to the tool's own process id. A process the tool started
#   is not signalled, and when the tool has ended while such a process still
#   holds its output open, the id that is signalled is no longer the tool's.
# - "timed out after N s" names the bound of the boundary, not the seconds
#   that measurement ran: one that started late in its boundary had fewer.
# - The shell's own file tests on the paths it measures are not bounded, and
#   the short processes that format a line are not in the seconds named.
#
# A figure from `du` is what the tool printed. Its exit status is not read
# (reading it means waiting for the tool), so the script cannot tell a total
# of everything from one `du` cut short on something it could not read: such
# figures are printed "as reported by du (unverified)", and the total over
# the per-crate directories names how many of them `du` reported.
SPACE_TOP_CRATES=5
# The seconds the script waits for the measurements of one boundary in all,
# before SPACE_KILL_GRACE and a second of clock rounding.
SPACE_READING_BOUND=60
# The seconds a measurement that outlived the bound gets to end on SIGTERM
# before it gets SIGKILL.
SPACE_KILL_GRACE=1

# Whether a measurement can be bounded here: space_bounded needs the process
# id of a process substitution, which bash has from 4.4.
space_bounded_available() {
    [[ "${BASH_VERSINFO[0]}" -gt 4 || ("${BASH_VERSINFO[0]}" -eq 4 && "${BASH_VERSINFO[1]}" -ge 4) ]]
}

# Runs "$@", one external tool, for what is left of the boundary's seconds
# and prints its standard output. 0: the tool's output ended in time (its own
# status is not read: reading it means waiting for it). 124: it was still
# running when the seconds were used up, and was signalled. 125: none were
# left, and it was not started.
#
# The tool's output comes through a descriptor of this shell, read line by
# line with a timeout: no pipeline and no command substitution waits on the
# tool itself. Past the bound the descriptor is closed first, the tool's
# process gets SIGTERM, then SIGKILL after SPACE_KILL_GRACE, and nothing
# waits for it. A tool that even SIGKILL does not end, or a process it
# started, stays behind holding only a pipe nobody reads (its input and
# error output are /dev/null), never this script's output.
space_bounded() {
    local fd="" pid="" line="" left=0 tries=0
    left=$((space_deadline - SECONDS))
    if [[ "$left" -le 0 ]]; then
        return 125
    fi
    exec {fd}< <(exec "$@" 2>/dev/null </dev/null)
    pid=$!
    while :; do
        left=$((space_deadline - SECONDS))
        if [[ "$left" -le 0 ]]; then
            exec {fd}<&-
            kill -TERM "$pid" 2>/dev/null
            while kill -0 "$pid" 2>/dev/null && [[ "$tries" -lt $((SPACE_KILL_GRACE * 10)) ]]; do
                sleep 0.1
                tries=$((tries + 1))
            done
            kill -KILL "$pid" 2>/dev/null
            return 124
        fi
        if IFS= read -r -t "$left" -u "$fd" line; then
            printf '%s\n' "$line"
        elif [[ "$?" -le 128 ]]; then
            # The end of the output; a last line may lack its newline.
            [[ -z "$line" ]] || printf '%s\n' "$line"
            break
        fi
    done
    exec {fd}<&-
    return 0
}

# For a measurement that was not taken ($3: space_bounded's status) prints
# its line and returns 0; returns 1 for one that was taken.
space_not_taken() {
    case "$3" in
        124)
            printf 'space reading [%s]: %s: timed out after %d s\n' "$1" "$2" "$SPACE_READING_BOUND"
            ;;
        125)
            printf 'space reading [%s]: %s: skipped: the time bound is used up\n' "$1" "$2"
            ;;
        *)
            return 1
            ;;
    esac
}

# One line per label and path pair after the boundary's name: the free and
# total space of the filesystem that holds the path (its nearest existing
# ancestor while the path does not exist). A filesystem an earlier pair
# already named is said to be the same one.
space_free_lines() {
    local boundary="$1" label="" path="" raw="" outcome=0 reading=""
    local free="" total="" id="" rest="" note="" index=0
    local -a seen_ids=() seen_labels=()
    shift
    while [[ "$#" -ge 2 ]]; do
        label="$1"
        path="$2"
        shift 2
        while [[ ! -e "$path" && "$path" == */* && "$path" != / ]]; do
            path="${path%/*}"
            [[ -n "$path" ]] || path=/
        done
        [[ -e "$path" ]] || path=.
        outcome=0
        raw="$(space_bounded df -Pk -- "$path")" || outcome=$?
        if space_not_taken "$boundary" "$label filesystem" "$outcome"; then
            continue
        fi
        reading="$(printf '%s\n' "$raw" | awk '
            NR == 2 && NF >= 6 && $(NF - 2) ~ /^[0-9]+$/ && $(NF - 4) ~ /^[0-9]+$/ {
                print $(NF - 2), $(NF - 4), $1 ":" $NF
            }')" || reading=""
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

# The size of one directory of this run, under a label. `du -skP`: no
# symbolic link is followed, on the command line or below it.
space_dir_line() {
    local boundary="$1" label="$2" dir="$3" raw="" outcome=0 size=""
    if [[ -L "$dir" || ! -d "$dir" ]]; then
        printf 'space reading [%s]: %s: no such directory\n' "$boundary" "$label"
        return 0
    fi
    raw="$(space_bounded du -skP -- "$dir")" || outcome=$?
    if space_not_taken "$boundary" "$label" "$outcome"; then
        return 0
    fi
    size="${raw%%$'\t'*}"
    if [[ ! "$size" =~ ^[0-9]+$ ]]; then
        printf 'space reading [%s]: %s: unavailable\n' "$boundary" "$label"
        return 0
    fi
    printf 'space reading [%s]: %s: %d MiB as reported by du (unverified)\n' "$boundary" "$label" "$((size / 1024))"
}

# The per-crate packaging directories of this run's crates (with whatever a
# persistent target already held in them): how many of the directories named
# to `du` it reported, their total, and the SPACE_TOP_CRATES largest by
# crate name, all as `du` reported them. One `du -skP` over all of them.
space_crate_lines() {
    local boundary="$1" crate="" raw="" outcome=0 sizes=""
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
    raw="$(space_bounded du -skP -- "${dirs[@]}")" || outcome=$?
    if space_not_taken "$boundary" "packaging target" "$outcome"; then
        return 0
    fi
    # "<KiB><TAB><crate name>" lines, largest first.
    sizes="$(printf '%s\n' "$raw" |
        awk -F '\t' 'NF >= 2 && $1 ~ /^[0-9]+$/ {
            name = $2
            sub(/\/+$/, "", name)
            sub(/.*\//, "", name)
            print $1 "\t" name
        }' |
        sort -rn)" || sizes=""
    if [[ -z "$sizes" ]]; then
        printf 'space reading [%s]: packaging target: unavailable\n' "$boundary"
        return 0
    fi
    printf '%s\n' "$sizes" |
        awk -F '\t' -v boundary="$boundary" -v top="$SPACE_TOP_CRATES" -v named="${#dirs[@]}" '
        NF == 2 {
            total += $1
            count += 1
            if (count <= top) {
                size[count] = $1
                name[count] = $2
            }
        }
        END {
            printf "space reading [%s]: packaging target, %d of %d per-crate directories of the release crates, whole contents: %d MiB as reported by du (unverified)\n", boundary, count, named, total / 1024
            shown = count < top ? count : top
            for (i = 1; i <= shown; i++) {
                printf "space reading [%s]: crate %d of %d by size: %s %d MiB as reported by du (unverified)\n", boundary, i, shown, name[i], size[i] / 1024
            }
        }'
}

# One boundary: free space, then what the boundary's name selects, then the
# seconds the readings took.
space_boundary() {
    local boundary="$1" started="$SECONDS"
    if ! space_bounded_available; then
        printf 'space reading [%s]: skipped: no bounded measurement available\n' "$boundary"
        return 0
    fi
    space_deadline=$((SECONDS + SPACE_READING_BOUND))
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
