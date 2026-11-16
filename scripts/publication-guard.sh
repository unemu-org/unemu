#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-or-later
# Keep tracked and untracked text safe to publish: the CPU core's other consumers must stay
# anonymous, and personal checkout paths, private hosts and private addresses do
# not belong in committed files.
#
# Run scripts/publication-guard.sh from any directory. Scan tracked files
# and untracked, non-ignored files, including docs and completed records;
# grep -I skips binary contents. Every check reads the text with LC_ALL=C and
# needs only bash 3.2, grep, sed and awk, so it runs the same on macOS and Linux
# without the devShell.
# The text checks skip the guard itself, because it defines the forbidden
# spellings, and Cargo.lock, which is generated dependency metadata. Cargo.lock
# has its own check: every dependency comes from crates.io or a github.com git
# repository. The private-hostname check reads both files too. The permitted
# x86jit forge URLs in Cargo.toml and the dev-setup SSH rewrite do not match the
# private filesystem path patterns; neither file needs a broader exemption.
#
# Checks:
# - private filesystem paths: home directories, macOS volumes and temporary
#   directories, Windows profiles, /root, and /mnt, /media and /run/media mounts;
# - e-mail addresses outside example domains and the allowlist below; the user
#   of a URL or of an scp-style remote (an SSH login) is not an e-mail address;
# - RFC 1918 and carrier-grade NAT (100.64.0.0/10) IPv4 addresses outside the
#   test values below; network ranges in CIDR notation are not addresses;
# - private hostnames, read only from the comma-separated environment variable
#   PUBLICATION_GUARD_PRIVATE_HOSTS so they never appear in the repository;
#   without it, this check is skipped, so it runs only where someone sets it,
#   never in CI. A hit prints only the file and line, never the name;
# - LGPL headers outside the OPL3 port, and the project's former name.
#
# The project was renamed from unemupc98 to unemupc, so the old name fails the
# guard too. It stays only where changing it would rewrite history or move a
# pinned hash: the decision, completed and archived records, and the status
# entries from the rename down; the remote repository URLs until those
# repositories are renamed; the quoted bytes of fixtures whose image or source
# archive is pinned; and the preferences module, which copies the file from
# the old directory.
set -euo pipefail
export LC_ALL=C

cd "$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

# Absolute private path forms. The ones without a user directory must start a
# path, so a URL or a relative path that merely contains them does not match.
path_start='(^|[^[:alnum:]_.~-])'
pattern='ps4|ps5|playstation|orbis|unemups4|/home/[^/[:space:]]+|/Users/[^/[:space:]]+|~/src/'
pattern+="|${path_start}(/volumes/|/private/(var|tmp)/|/var/folders/|/root/|/mnt/[^/[:space:]]+"
pattern+='|/media/[^/[:space:]]+/|/run/media/[^/[:space:]]+/)'
pattern+='|[[:alpha:]]:[/\\]+users[/\\]'
old_name='unemupc98'
# An address preceded by '/' is a URL's user information, not an e-mail address.
# The optional tail catches the ':path' of an scp-style remote, which the
# report then skips.
email_re='(^|[^/[:alnum:]._%+-])[[:alnum:]._%+-]+@[[:alnum:]-]+(\.[[:alnum:]-]+)*\.[[:alpha:]]{2,}(:[[:alnum:]_.~/-])?'
email_remote=':[^:]$'
# The maintainer's public address and the commit trailer of the coding agent.
email_allowed='^(mikolaj@mikolajczyk\.org|noreply@anthropic\.com|.*@(([[:alnum:]-]+\.)*example\.(org|com|net)|([[:alnum:]-]+\.)+example))$'
# RFC 1918 and carrier-grade NAT; an optional prefix length marks a CIDR range,
# which the report skips.
private_ip_re='(^|[^[:alnum:].])(10(\.[0-9]{1,3}){3}|172\.(1[6-9]|2[0-9]|3[01])(\.[0-9]{1,3}){2}|192\.168(\.[0-9]{1,3}){2}'
private_ip_re+='|100\.(6[4-9]|[7-9][0-9]|1[01][0-9]|12[0-7])(\.[0-9]{1,3}){2})(/[0-9]{1,2})?($|[^0-9.]|\.$|\.[^0-9])'
private_ip_range='[0-9]/[0-9]'
# Test values for "not a loopback host" in the registry client.
private_ip_allowed='^(10\.0\.0\.1)$'
lock_source_allowed='^source = "(registry\+https://github\.com/rust-lang/crates\.io-index|sparse\+https://index\.crates\.io/|git\+https://github\.com/[^"[:space:]]+)"$'
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

all_files=()
files=()
while IFS= read -r -d '' file; do
  all_files+=("$file")
  case "$file" in
    scripts/publication-guard.sh|Cargo.lock) ;;
    *) files+=("$file") ;;
  esac
done < <(git ls-files --cached --others --exclude-standard -z)

# Print the hits of one check whose matches are filtered by an allowlist:
# grep's "file:line:match" lines on stdin; matches containing the ERE in $3
# are not values at all; the rest are reduced to the bare value by the pattern
# in $1, then values matching the ERE in $2 are dropped.
# The patterns travel in the environment: awk -v would rewrite their backslashes.
report_unlisted() {
  GUARD_VALUE="$1" GUARD_ALLOWED="$2" GUARD_SKIP="$3" awk '{
    text = $0
    sub(/^[^:]*:[0-9]*:/, "", text)
    if (text ~ ENVIRON["GUARD_SKIP"]) next
    if (!match(text, ENVIRON["GUARD_VALUE"])) next
    found = tolower(substr(text, RSTART, RLENGTH))
    if (found ~ ENVIRON["GUARD_ALLOWED"]) next
    print
  }'
}

# Run grep over every scanned file at once; status 1 means no match.
scan() {
  local status=0
  LC_ALL=C grep "$@" || status=$?
  if (( status > 1 )); then
    exit "$status"
  fi
}

for file in "${files[@]}"; do

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
done

hits=$(scan -HIinoE -- "$email_re" "${files[@]}" |
  report_unlisted '[[:alnum:]._%+-]+@[[:alnum:].-]*[[:alnum:]]' "$email_allowed" "$email_remote")
if [[ -n $hits ]]; then
  printf '%s\n' "$hits"
  printf '%s\n' 'publication guard: e-mail address outside the allowlist' >&2
  rc=1
fi

hits=$(scan -HIinoE -- "$private_ip_re" "${files[@]}" |
  report_unlisted '[0-9]+[.][0-9]+[.][0-9]+[.][0-9]+' "$private_ip_allowed" "$private_ip_range")
if [[ -n $hits ]]; then
  printf '%s\n' "$hits"
  printf '%s\n' 'publication guard: private IPv4 address outside the allowlist' >&2
  rc=1
fi

host_patterns=()
IFS=',' read -r -a hosts <<< "${PUBLICATION_GUARD_PRIVATE_HOSTS:-}"
for host in ${hosts[@]+"${hosts[@]}"}; do
  host=$(printf '%s' "$host" | LC_ALL=C sed -E 's/^[[:space:]]+//; s/[[:space:]]+$//')
  if [[ -n $host ]]; then
    host_patterns+=(-e "$host")
  fi
done
if (( ${#host_patterns[@]} )); then
  # Only file and line: the names stay out of logs and terminals.
  hits=$(scan -HIinF "${host_patterns[@]}" -- "${all_files[@]}" | cut -d: -f1,2)
  if [[ -n $hits ]]; then
    printf '%s\n' "$hits"
    printf '%s\n' 'publication guard: private hostname from PUBLICATION_GUARD_PRIVATE_HOSTS' >&2
    rc=1
  fi
fi

if [[ -f Cargo.lock ]]; then
  hits=$(scan -n '^source = ' Cargo.lock |
    LC_ALL=C GUARD_ALLOWED="$lock_source_allowed" awk '{
      text = $0
      sub(/^[0-9]*:/, "", text)
      if (text !~ ENVIRON["GUARD_ALLOWED"]) print "Cargo.lock:" $0
    }')
  if [[ -n $hits ]]; then
    printf '%s\n' "$hits"
    printf '%s\n' 'publication guard: Cargo.lock source outside crates.io and github.com' >&2
    rc=1
  fi
fi

if (( rc )); then
  printf '%s\n' 'publication guard: fix the publication or licence diagnostics above.' >&2
fi
exit "$rc"
