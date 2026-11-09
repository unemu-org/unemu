#!/usr/bin/env bash
# Keep tracked text safe to publish: the CPU core's other consumers must stay
# anonymous, and personal checkout paths do not belong in committed files.
#
# Run scripts/publication-guard.sh from any directory. Scan tracked working-tree
# files, including docs and completed records; grep -I skips binary contents.
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

  if hits=$(grep -HIniE -- "$pattern" "$file"); then
    printf '%s\n' "$hits"
    rc=1
  else
    grep_status=$?
    if (( grep_status != 1 )); then
      exit "$grep_status"
    fi
  fi
done < <(git ls-files -z)

if (( rc )); then
  printf '%s\n' 'publication guard: replace consumer names and personal paths with generic wording.' >&2
fi
exit "$rc"
