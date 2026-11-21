#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-or-later
# Keep the author's archive intact: recipe conformance verifies every member
# and reproduces the pinned disk from it, without fetching inputs itself.
set -euo pipefail

if command -v sha256sum >/dev/null 2>&1; then
	SHA256=(sha256sum)
elif command -v shasum >/dev/null 2>&1; then
	SHA256=(shasum -a 256)
else
	echo "missing SHA-256 tool: install sha256sum or shasum (or use nix develop)" >&2
	exit 1
fi

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
OUT="$HERE/LABFULL.ZIP"
URL="https://advsys.net/ken/klab/labfull.zip"
SIZE=940242
SHA256_PIN="643aa0ca8d1a7d2c88afecd952da00f3b912be8b83c4c1d2f4ab9b50491fadfd"

verify() {
	local file="$1" size digest
	size="$(wc -c < "$file" | tr -d '[:space:]')"
	digest="$("${SHA256[@]}" "$file" | awk '{print $1}')"
	if [ "$size" != "$SIZE" ] || [ "$digest" != "$SHA256_PIN" ]; then
		echo "LABFULL.ZIP mismatch: expected $SIZE bytes, SHA-256 $SHA256_PIN; got $size bytes, SHA-256 $digest" >&2
		echo "Remove the cached archive and fetch the pinned release again; do not change its pins." >&2
		return 1
	fi
}

if [ -f "$OUT" ]; then
	verify "$OUT"
else
	trap 'rm -f "$OUT.part"' EXIT
	curl -fsSL -o "$OUT.part" "$URL"
	verify "$OUT.part"
	mv "$OUT.part" "$OUT"
fi
echo "LABFULL.ZIP matches its size and SHA-256 pins"
