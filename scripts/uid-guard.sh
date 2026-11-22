#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-or-later
# uid-guard — pre-commit hook that refuses to sign when user.email has no
# matching UID on user.signingkey. Vendored from the home-manager template
# so the repo carries its own copy independent of any contributor's host.

set -e

if [[ ${1:-} == --self-test ]]; then
  scratch=$(mktemp -d)
  trap 'rm -rf "$scratch"' EXIT
  mkdir -p "$scratch/bin"
  cp "${BASH_SOURCE[0]}" "$scratch/uid-guard.sh"
  cat > "$scratch/bin/git" <<'GIT'
#!/usr/bin/env bash
case "$*" in
  'config --get commit.gpgsign') echo "${UID_TEST_SIGN:-true}" ;;
  'config --get user.email') echo "${UID_TEST_EMAIL-user@example.org}" ;;
  'config --get user.signingkey') echo "${UID_TEST_KEY-test-key}" ;;
  *) exit 2 ;;
esac
GIT
  cat > "$scratch/bin/gpg" <<'GPG'
#!/usr/bin/env bash
printf 'uid:::::::::Test <%s>:\n' "${UID_TEST_UID:-user@example.org}"
GPG
  chmod +x "$scratch/bin/git" "$scratch/bin/gpg"
  export PATH="$scratch/bin:$PATH"
  for test_case in unsigned no-email no-key match case-match mismatch; do
    status=0
    (
      unset UID_TEST_SIGN UID_TEST_EMAIL UID_TEST_KEY UID_TEST_UID
      case "$test_case" in
        unsigned) export UID_TEST_SIGN=false ;;
        no-email) export UID_TEST_EMAIL= ;;
        no-key) export UID_TEST_KEY= ;;
        case-match) export UID_TEST_UID=USER@example.org ;;
        mismatch) export UID_TEST_UID=other@example.org ;;
      esac
      bash "$scratch/uid-guard.sh"
    ) > "$scratch/result" 2>&1 || status=$?
    if [[ $test_case == mismatch ]]; then
      [[ $status == 1 ]] && grep -q 'UID mismatch' "$scratch/result"
    else
      [[ $status == 0 ]]
    fi
  done
  echo 'uid-guard: self-test passed (6 cases)'
  exit 0
fi

if ! git config --get commit.gpgsign | grep -qE '^(true|1)$'; then
  exit 0
fi

email=$(git config --get user.email || true)
signingkey=$(git config --get user.signingkey || true)

if [ -z "$email" ] || [ -z "$signingkey" ]; then
  exit 0
fi

key_ref=${signingkey%!}
matched_uid=$(gpg --list-keys --with-colons "$key_ref" 2>/dev/null \
  | awk -F: -v e="$email" 'tolower($1)=="uid" && index(tolower($10), tolower(e)) { print $10; exit }')

if [ -z "$matched_uid" ]; then
  cat >&2 <<EOF
✗ pre-commit blocked: signing-key UID mismatch.

  user.email      : $email
  user.signingkey : $signingkey
  primary key UIDs:
EOF
  gpg --list-keys --with-colons "$key_ref" 2>/dev/null \
    | awk -F: '$1=="uid" { print "    - " $10 }' >&2
  cat >&2 <<EOF

  No UID on this key contains "$email".
  Either fix user.email / user.signingkey, or attach a matching UID to the key.
EOF
  exit 1
fi

exit 0
