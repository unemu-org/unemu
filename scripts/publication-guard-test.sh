#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-or-later
# Deleted tracked files must not hide findings in files still being published.
set -euo pipefail
root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
scratch=$(mktemp -d)
trap 'rm -rf "$scratch"' EXIT
mkdir -p "$scratch/scripts" "$scratch/bin"
cp "$root/scripts/publication-guard.sh" "$scratch/scripts/"
cat > "$scratch/bin/git" <<'SH'
#!/usr/bin/env bash
printf '%s\0' kept.txt deleted.txt new.txt
SH
chmod +x "$scratch/bin/git"
export PATH="$scratch/bin:$PATH"
unset PUBLICATION_GUARD_PRIVATE_HOSTS
: > "$scratch/kept.txt"
: > "$scratch/new.txt"
bash "$scratch/scripts/publication-guard.sh" > "$scratch/result" 2>&1 || {
  cat "$scratch/result"
  exit 1
}
for file in kept.txt new.txt; do
  printf '%s%s\n' 'dev@' 'invalid.test' > "$scratch/$file"
  if bash "$scratch/scripts/publication-guard.sh" > "$scratch/result" 2>&1; then
    echo "FAIL: guard accepted $file" >&2
    exit 1
  fi
  grep -q "$file:1:" "$scratch/result"
  : > "$scratch/$file"
done
echo 'PASS: deleted tracked file skipped; tracked and untracked findings rejected'
