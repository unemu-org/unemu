#!/usr/bin/env bash
# Keep tracked and untracked text safe to publish: the CPU core's other consumers must stay
# anonymous, and personal checkout paths do not belong in committed files.
#
# Run scripts/publication-guard.sh from any directory. Scan tracked files
# and untracked, non-ignored files, including docs and completed records;
# grep -I skips binary contents.
# The guard excludes itself because it defines the forbidden spellings, and
# Cargo.lock is excluded as generated dependency metadata. The permitted x86jit
# forge URLs in Cargo.toml and the dev-setup SSH rewrite do not match the private
# filesystem path patterns; neither file needs a broader exemption.
set -euo pipefail

cd "$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

pattern='ps4|ps5|playstation|orbis|unemups4|/home/[^/[:space:]]+|/Users/[^/[:space:]]+|~/src/'
rc=0

while IFS= read -r -d '' file; do
  case "$file" in
    scripts/publication-guard.sh|Cargo.lock) continue ;;
  esac

  # The LGPL translation is deliberately limited to the two upstream-derived
  # files. Assembly, board timing, snapshots and tests retain the project licence.
  if grep -IqE '^// SPDX-License-Identifier: LGPL-' "$file"; then
    case "$file" in
      crates/devices/src/opl3/mod.rs|crates/devices/src/opl3/tables.rs)
        if [[ $(head -n 1 "$file") != '// SPDX-License-Identifier: LGPL-2.1-or-later' ]]; then
          printf '%s\n' "publication guard: invalid OPL3 licence: $file" >&2
          rc=1
        fi ;;
      *) printf '%s\n' "publication guard: LGPL outside the OPL3 port: $file" >&2; rc=1 ;;
    esac
  fi

  if hits=$(grep -HIniE -- "$pattern" "$file"); then
    printf '%s\n' "$hits"
    rc=1
  else
    grep_status=$?
    if (( grep_status != 1 )); then
      exit "$grep_status"
    fi
  fi
done < <(git ls-files --cached --others --exclude-standard -z)

if (( rc )); then
  printf '%s\n' 'publication guard: fix the publication or licence diagnostics above.' >&2
fi
exit "$rc"
