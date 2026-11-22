#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-or-later
# Keep the extraction inventory complete and disjoint, including shared files.
# Patterns and their destinations follow backlog/docs/design/repo-split.md §2.
set -euo pipefail
export LC_ALL=C
cd "$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

owners=scripts/repo-ownership.paths
files=
self_test=0
while (( $# )); do
  case "$1" in
    --owners) owners=$2; shift 2 ;;
    --files) files=$2; shift 2 ;;
    --self-test) self_test=1; shift ;;
    *) echo 'Usage: bash scripts/repo-ownership.sh [--self-test] [--owners FILE] [--files FILE]' >&2; exit 2 ;;
  esac
done

check() {
  local inventory=$1 paths=$2 owner pattern path matches destination i status=0
  local core=0 frontend=0 shared=0
  local destinations=() patterns=()
  while read -r owner pattern || [ -n "$owner" ]; do
    case "$owner" in ''|'#'*) continue ;; esac
    case "$owner" in
      core|frontend|shared) ;;
      *) echo "ownership: invalid destination: $owner" >&2; return 2 ;;
    esac
    if [[ -z $pattern ]]; then
      echo "ownership: missing pattern for $owner" >&2
      return 2
    fi
    destinations+=("$owner")
    patterns+=("$pattern")
  done < "$inventory"
  while IFS= read -r -d '' path; do
    matches=0
    destination=
    for (( i=0; i<${#patterns[@]}; i++ )); do
      if [[ $path == ${patterns[i]} ]]; then
        matches=$((matches + 1))
        destination=${destinations[i]}
      fi
    done
    case "$matches" in
      0) echo "ownership: unowned: $path" >&2; status=1 ;;
      1)
        case "$destination" in
          core) core=$((core + 1)) ;;
          frontend) frontend=$((frontend + 1)) ;;
          shared) shared=$((shared + 1)) ;;
        esac ;;
      *) echo "ownership: multiply owned ($matches matches): $path" >&2; status=1 ;;
    esac
  done < "$paths"
  echo "ownership: core=$core frontend=$frontend shared=$shared"
  return "$status"
}

scratch=$(mktemp -d)
trap 'rm -rf "$scratch"' EXIT
if (( self_test )); then
  printf 'core crates/core/*\nfrontend crates/front/*\nshared Cargo.toml\n' > "$scratch/owners"
  printf '%s\0' crates/core/src/lib.rs crates/front/src/lib.rs Cargo.toml > "$scratch/files"
  check "$scratch/owners" "$scratch/files"
  printf '%s\0' new-family/stray.txt >> "$scratch/files"
  if check "$scratch/owners" "$scratch/files" > "$scratch/result" 2>&1; then
    echo 'FAIL ownership accepted an unowned file' >&2; exit 1
  fi
  grep -F 'ownership: unowned: new-family/stray.txt' "$scratch/result"
  printf '%s\0' crates/core/src/lib.rs crates/front/src/lib.rs Cargo.toml > "$scratch/files"
  for owner in frontend core shared; do
    cp "$scratch/owners" "$scratch/overlap"
    printf '%s crates/core/src/lib.rs\n' "$owner" >> "$scratch/overlap"
    if check "$scratch/overlap" "$scratch/files" > "$scratch/result" 2>&1; then
      echo "FAIL ownership accepted overlap with $owner" >&2; exit 1
    fi
    grep -F 'ownership: multiply owned (2 matches): crates/core/src/lib.rs' "$scratch/result"
  done
  # A last line without a newline is still read.
  cp "$scratch/owners" "$scratch/overlap"
  printf 'frontend crates/core/src/lib.rs' >> "$scratch/overlap"
  if check "$scratch/overlap" "$scratch/files" > "$scratch/result" 2>&1; then
    echo 'FAIL ownership ignored an unterminated last line' >&2; exit 1
  fi
  grep -F 'ownership: multiply owned (2 matches): crates/core/src/lib.rs' "$scratch/result"
  echo 'ownership self-test: passed (unowned, cross-owner, same-owner, shared and unterminated overlaps rejected)'
else
  if [[ -n $files ]]; then
    # Explicit file lists use the planner's newline-delimited format.
    while IFS= read -r path || [ -n "$path" ]; do printf '%s\0' "$path"; done < "$files" > "$scratch/files"
  else
    # Include new files so an unstaged addition receives the same review as a tracked one.
    git ls-files --cached --others --exclude-standard -z > "$scratch/files"
  fi
  check "$owners" "$scratch/files"
fi
