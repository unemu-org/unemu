#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-or-later
# Exercise installation of the current binary before a release exists. Only
# curl is substituted; the installer still checks and unpacks the real archive.
set -euo pipefail
binary=$1
fixture=$2
case "$(uname -s)/$(uname -m)" in
  Linux/x86_64) target=x86_64-unknown-linux-gnu ;;
  Darwin/arm64) target=aarch64-apple-darwin ;;
  *) echo 'unsupported action test platform' >&2; exit 1 ;;
esac
name=unemu-0.0.0-test-$target
mkdir -p "$fixture/archive/$name" "$fixture/bin"
cp "$binary" LICENSE "$fixture/archive/$name/"
# Only installation is under test; release.yml generates the full notice.
printf 'test notice\n' > "$fixture/archive/$name/THIRD-PARTY-LICENSES.md"
tar -c -z -f "$fixture/$name.tar.gz" -C "$fixture/archive" "$name"
if command -v sha256sum >/dev/null; then
  hash=$(sha256sum < "$fixture/$name.tar.gz")
else
  hash=$(shasum -a 256 < "$fixture/$name.tar.gz")
fi
printf '%s  %s.tar.gz\n' "${hash%% *}" "$name" > "$fixture/SHA256SUMS"
cat > "$fixture/bin/curl" <<'CURL'
#!/usr/bin/env bash
set -euo pipefail
output=
while (( $# )); do
  case "$1" in
    --output) output=$2; shift 2 ;;
    https://*) url=$1; shift ;;
    *) shift ;;
  esac
done
[[ $url == https://github.com/unemu-org/unemu/releases/download/v0.0.0-test/* ]] || exit 22
cp "$SETUP_UNEMU_FIXTURE/${url##*/}" "$output"
CURL
chmod +x "$fixture/bin/curl"
