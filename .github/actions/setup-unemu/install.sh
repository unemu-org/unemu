#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-or-later
# The install step of the setup-unemu action: download one release archive of
# unemu, check it against the release's own SHA256SUMS (a check of the
# download, not a pin), and install unemu from it into the runner's temporary
# directory.
#
# The inputs arrive only through the environment (UNEMU_VERSION,
# UNEMU_COMMIT), never pasted into the script text, so no value can become
# shell code. The runner provides RUNNER_OS, RUNNER_ARCH, RUNNER_TEMP,
# GITHUB_PATH and GITHUB_OUTPUT. An invalid input is described, never echoed:
# a printed line that starts with :: would be read as a workflow command.
#
# It needs bash 3.2, curl, tar, and sha256sum or shasum, so it runs the same on
# the hosted Linux and macOS runners and by hand on a Mac.
set -euo pipefail

error() {
  printf '::error::%s\n' "$*"
  exit 1
}

version=${UNEMU_VERSION:-}
commit=${UNEMU_COMMIT:-}

version_re='^v[0-9][0-9A-Za-z.+-]*$'
commit_re='^[0-9a-f]{40}$'
if [[ ! $version =~ $version_re ]]; then
  error 'version must be a release tag of unemu, such as v0.0.4'
fi
if [[ -n $commit && ! $commit =~ $commit_re ]]; then
  error 'commit must be empty or a full commit: 40 lowercase hex digits'
fi

# The platforms the release workflow builds for.
case "${RUNNER_OS:-}/${RUNNER_ARCH:-}" in
  Linux/X64) target=x86_64-unknown-linux-gnu ;;
  macOS/ARM64) target=aarch64-apple-darwin ;;
  *) error "unemu releases are available for Linux X64 and macOS ARM64 runners, not for ${RUNNER_OS:-unknown}/${RUNNER_ARCH:-unknown}" ;;
esac

: "${RUNNER_TEMP:?}" "${GITHUB_PATH:?}" "${GITHUB_OUTPUT:?}"

name=unemu-${version#v}-$target
archive=$name.tar.gz
base=https://github.com/unemu-org/unemu/releases/download/$version

# The archive never outlives this step, whether it passes the check or not.
download=$(mktemp -d "$RUNNER_TEMP/setup-unemu-download.XXXXXXXX")
trap 'rm -rf "$download"' EXIT

fetch() {
  curl --fail --silent --show-error --location \
    --proto '=https' --proto-redir '=https' \
    --retry 5 --connect-timeout 30 \
    --output "$download/$1" "$base/$1"
}
echo "downloading $base/$archive"
fetch "$archive"
fetch SHA256SUMS
sha256=$(awk -v name="$archive" '$2 == name || $2 == "*" name { print $1 }' "$download/SHA256SUMS")
if [[ ! $sha256 =~ ^[0-9a-f]{64}$ ]]; then
  error "SHA256SUMS of $version lists no checksum for $archive; nothing was installed"
fi

# Hash from standard input: with a file name argument, both tools escape
# unusual names in their output.
if command -v sha256sum > /dev/null; then
  actual=$(sha256sum < "$download/$archive")
else
  actual=$(shasum -a 256 < "$download/$archive")
fi
actual=${actual%% *}
if [[ $actual != "$sha256" ]]; then
  error "$archive has SHA-256 $actual, but the release's SHA256SUMS lists $sha256; nothing was installed"
fi
echo "$archive matches the release's SHA256SUMS ($sha256)"

# The combined front end and its licence texts; the README and repository
# notices stay in the archive.
bin=$(mktemp -d "$RUNNER_TEMP/unemu-$version.XXXXXXXX")
tar -x -z -f "$download/$archive" -C "$bin" --strip-components 1 \
  "$name/unemu" "$name/LICENSE" "$name/THIRD-PARTY-LICENSES.md"
unemu=$bin/unemu

about=$("$unemu" --version)
printf '%s\n' "$about"

# A tag can be moved after the fact, a commit cannot; release builds name
# theirs on a "commit:" line of --version.
if [[ -n $commit ]]; then
  built=$(printf '%s\n' "$about" | sed -n 's/^commit: \([0-9a-f]\{40\}\)$/\1/p')
  if [[ -z $built ]]; then
    error "unemu $version prints no commit line, so it cannot be checked against the pinned commit $commit; leave commit empty for this release"
  fi
  if [[ $built != "$commit" ]]; then
    error "unemu $version was built from commit $built, not the pinned $commit"
  fi
  echo "unemu $version was built from the pinned commit $commit"
fi

echo "$bin" >> "$GITHUB_PATH"
echo "path=$unemu" >> "$GITHUB_OUTPUT"
