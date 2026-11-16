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
#
# The project was renamed from unemupc98 to unemupc, so the old name fails the
# guard too. It stays only where changing it would rewrite history or move a
# pinned hash: the decision, completed and archived records, and the status
# entries from the rename down; the remote repository URLs until those
# repositories are renamed; the quoted bytes of fixtures whose image or source
# archive is pinned; and the preferences module, which copies the file from
# the old directory.
set -euo pipefail

cd "$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

pattern='ps4|ps5|playstation|orbis|unemups4|/home/[^/[:space:]]+|/Users/[^/[:space:]]+|~/src/'
old_name='unemupc98'
rc=0

old_name_allowed() {
  case "$1" in
    backlog/decisions/*|backlog/completed/*|backlog/archive/*) return 0 ;;
    # Copied into the source archives whose SHA-256 the firmware catalogue pins.
    vendor/*/source-notice.txt) return 0 ;;
    # LICENSES.TXT of the FreeDOS 1.4 template, whose hash is pinned.
    vendor/freedos/floppy-1.4-licenses.txt) return 0 ;;
    crates/cli/src/preferences.rs) return 0 ;;
  esac
  return 1
}

# The text of a file the old-name check reads, without repository URLs. In
# the status log that is every line above the rename entry: new entries go on
# top, while the older ones record what the program was called then. Without
# the rename entry the whole log is read, so the guard fails rather than
# skipping it. In the fixture sources it leaves out only the quoted bytes:
# the hard-disk fixture's MARKER.TXT, which the guest reads, and the SeaBIOS
# version suffix inside the ROM, whose hash is pinned.
old_name_text() {
  local urls='s#[[:alpha:]][[:alnum:]+.-]*://[^[:space:]"<>]*##g'
  case "$1" in
    backlog/docs/status.md)
      LC_ALL=C awk 'index($0, "The project is renamed unemupc:**") { exit } { print }' "$1" |
        LC_ALL=C sed -E "$urls" ;;
    vendor/hdd/build.sh|vendor/hdd/README.md)
      LC_ALL=C sed -E -e "$urls" -e 's#unemupc98 hard disk fixture##g' "$1" ;;
    vendor/seabios/fetch.sh|vendor/seabios/README.md)
      LC_ALL=C sed -E -e "$urls" -e 's#-unemupc98-isa##g' "$1" ;;
    *) LC_ALL=C sed -E "$urls" "$1" ;;
  esac
}

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

  if ! old_name_allowed "$file" && LC_ALL=C grep -IqiF -- "$old_name" "$file"; then
    if ! text=$(old_name_text "$file"); then
      printf '%s\n' "publication guard: cannot read $file for the old name" >&2
      exit 2
    fi
    if hits=$(printf '%s\n' "$text" | LC_ALL=C grep -HniF --label="$file" -- "$old_name"); then
      printf '%s\n' "$hits"
      printf '%s\n' "publication guard: old project name $old_name: $file" >&2
      rc=1
    else
      grep_status=$?
      if (( grep_status != 1 )); then
        exit "$grep_status"
      fi
    fi
  fi
done < <(git ls-files --cached --others --exclude-standard -z)

if (( rc )); then
  printf '%s\n' 'publication guard: fix the publication or licence diagnostics above.' >&2
fi
exit "$rc"
