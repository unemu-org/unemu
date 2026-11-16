#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-or-later
# The install step of the setup-upcli action: download one release archive of
# unemupc, check it against the SHA-256 the caller pinned, and install upcli
# from it into the runner's temporary directory.
#
# The inputs arrive only through the environment (UPCLI_VERSION, UPCLI_SHA256,
# UPCLI_COMMIT), never pasted into the script text, so no value can become
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

version=${UPCLI_VERSION:-}
sha256=${UPCLI_SHA256:-}
commit=${UPCLI_COMMIT:-}

version_re='^v[0-9][0-9A-Za-z.+-]*$'
sha256_re='^[0-9a-f]{64}$'
commit_re='^[0-9a-f]{40}$'
if [[ ! $version =~ $version_re ]]; then
  error 'version must be a release tag of unemupc, such as v0.0.1'
fi
if [[ ! $sha256 =~ $sha256_re ]]; then
  error 'sha256 must be the SHA-256 of the release archive: 64 lowercase hex digits'
fi
if [[ -n $commit && ! $commit =~ $commit_re ]]; then
  error 'commit must be empty or a full commit: 40 lowercase hex digits'
fi

# The platforms the release workflow builds for.
case "${RUNNER_OS:-}/${RUNNER_ARCH:-}" in
  Linux/X64) target=x86_64-unknown-linux-gnu ;;
  macOS/ARM64) target=aarch64-apple-darwin ;;
  *) error "unemupc releases upcli for Linux X64 and macOS ARM64 runners, not for ${RUNNER_OS:-unknown}/${RUNNER_ARCH:-unknown}" ;;
esac

: "${RUNNER_TEMP:?}" "${GITHUB_PATH:?}" "${GITHUB_OUTPUT:?}"

name=unemupc-${version#v}-$target
archive=$name.tar.gz
url=https://github.com/unemu-org/unemupc/releases/download/$version/$archive

# The archive never outlives this step, whether it passes the check or not.
download=$(mktemp -d "$RUNNER_TEMP/setup-upcli-download.XXXXXXXX")
trap 'rm -rf "$download"' EXIT

echo "downloading $url"
curl --fail --silent --show-error --location \
  --proto '=https' --proto-redir '=https' \
  --retry 5 --connect-timeout 30 \
  --output "$download/$archive" "$url"

# Hash from standard input: with a file name argument, both tools escape
# unusual names in their output.
if command -v sha256sum > /dev/null; then
  actual=$(sha256sum < "$download/$archive")
else
  actual=$(shasum -a 256 < "$download/$archive")
fi
actual=${actual%% *}
if [[ $actual != "$sha256" ]]; then
  error "$archive has SHA-256 $actual, not the pinned $sha256; nothing was installed"
fi
echo "$archive matches the pinned SHA-256 $sha256"

# Only the command line and the licence texts that go with it: the desktop
# app, the README and the notices stay in the archive.
bin=$(mktemp -d "$RUNNER_TEMP/upcli-$version.XXXXXXXX")
tar -x -z -f "$download/$archive" -C "$bin" --strip-components 1 \
  "$name/upcli" "$name/LICENSE" "$name/THIRD-PARTY-LICENSES.md"
upcli=$bin/upcli

about=$("$upcli" --version)
printf '%s\n' "$about"

# A tag can be moved after the fact, a commit cannot; release builds name
# theirs on a "commit:" line of --version.
if [[ -n $commit ]]; then
  built=$(printf '%s\n' "$about" | sed -n 's/^commit: \([0-9a-f]\{40\}\)$/\1/p')
  if [[ -z $built ]]; then
    error "upcli $version prints no commit line, so it cannot be checked against the pinned commit $commit; leave commit empty for this release"
  fi
  if [[ $built != "$commit" ]]; then
    error "upcli $version was built from commit $built, not the pinned $commit"
  fi
  echo "upcli $version was built from the pinned commit $commit"
fi

echo "$bin" >> "$GITHUB_PATH"
echo "path=$upcli" >> "$GITHUB_OUTPUT"
