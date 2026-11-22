#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-or-later
# Build fixtures with the tools belonging to the core revision Cargo uses.
set -euo pipefail

fail() {
    echo "core-checkout: $*" >&2
    exit 1
}

lock_revision() {
    local source revision="" candidate resolved
    local pattern='^git[+]https://github[.]com/unemu-org/unemupc[?]rev=([[:xdigit:]]{40})#([[:xdigit:]]{40})$'
    while IFS= read -r source; do
        [[ $source =~ $pattern ]] || fail "unemupc source must use rev=<40 hex>#<40 hex>: $source"
        candidate=$(printf '%s' "${BASH_REMATCH[1]}" | tr 'A-F' 'a-f')
        resolved=$(printf '%s' "${BASH_REMATCH[2]}" | tr 'A-F' 'a-f')
        [[ $candidate == "$resolved" ]] || fail "unemupc requested and resolved revisions differ"
        [[ -z $revision || $revision == "$candidate" ]] || fail "Cargo.lock contains mixed unemupc revisions"
        revision=$candidate
    done < <(awk '
        /^[[:space:]]*source[[:space:]]*=/ {
            sub(/^[^"]*"/, ""); sub(/"[[:space:]]*$/, "")
            if (tolower($0) ~ /github[.]com[:\/]unemu-org\/unemupc([.\/?#]|$)/) print
        }
    ' "$1")
    [[ -n $revision ]] || fail "Cargo.lock contains no unemupc git packages"
    printf '%s\n' "$revision"
}

self_test() (
    local scratch first second result case_name spelling
    scratch=$(mktemp -d)
    scratch=$(cd -- "$scratch" && pwd -P)
    trap 'rm -rf "$scratch"' EXIT
    first=1111111111111111111111111111111111111111
    second=2222222222222222222222222222222222222222
    plant() {
        printf '[[package]]\nname = "core-test"\nsource = "git+https://github.com/unemu-org/unemupc?rev=%s#%s"\n' "$1" "$2"
    }
    { plant "$first" "$first"; plant "$first" "$first"; } > "$scratch/one.lock"
    result=$(lock_revision "$scratch/one.lock")
    [[ $result == "$first" ]] || fail "self-test: one revision was not accepted"
    printf 'version = 4\n' > "$scratch/none.lock"
    { plant "$first" "$first"; plant "$second" "$second"; } > "$scratch/mixed.lock"
    plant short "$first" > "$scratch/short.lock"
    plant main "$first" > "$scratch/branch.lock"
    plant v1.0 "$first" > "$scratch/tag.lock"
    plant "$first" "$second" > "$scratch/resolved.lock"
    for spelling in git slash branch_only ssh case; do
        plant "$first" "$first" > "$scratch/$spelling.lock"
        case $spelling in
            git) printf 'source = "git+https://github.com/unemu-org/unemupc.git?rev=%s#%s"\n' "$second" "$second" ;;
            slash) printf 'source = "git+https://github.com/unemu-org/unemupc/?rev=%s#%s"\n' "$second" "$second" ;;
            branch_only) printf 'source = "git+https://github.com/unemu-org/unemupc#%s"\n' "$second" ;;
            ssh) printf 'source = "git+ssh://git@github.com/unemu-org/unemupc?rev=%s#%s"\n' "$second" "$second" ;;
            case) printf 'source = "git+https://GitHub.com/Unemu-Org/Unemupc?rev=%s#%s"\n' "$second" "$second" ;;
        esac >> "$scratch/$spelling.lock"
    done
    for case_name in none mixed short branch tag resolved git slash branch_only ssh case; do
        if (lock_revision "$scratch/$case_name.lock") > /dev/null 2> "$scratch/error"; then
            fail "self-test: $case_name lock was accepted"
        fi
        [[ -s $scratch/error ]] || fail "self-test: $case_name failed without an explanation"
    done
    mkdir -p "$scratch/scripts" "$scratch/bin" "$scratch/.core/unemupc/.git"
    cp -- "${BASH_SOURCE[0]}" "$scratch/scripts/core-checkout.sh"
    plant "$first" "$first" > "$scratch/Cargo.lock"
    printf '%s\n' "$first" > "$scratch/.core/unemupc/.git/test-head"
    cat > "$scratch/bin/git" <<'GIT'
#!/usr/bin/env bash
set -euo pipefail
checkout=$2
shift 2
case "$*" in
    'rev-parse --show-toplevel') printf '%s\n' "$checkout" ;;
    'rev-parse HEAD') cat "$checkout/.git/test-head" ;;
    'status --porcelain --untracked-files=all') cat "$checkout/.git/test-status" ;;
    'cat-file -e '*|'checkout --detach '*) ;;
    *) exit 2 ;;
esac
GIT
    cat > "$scratch/bin/nix" <<'NIX'
#!/usr/bin/env bash
set -euo pipefail
[[ $* == 'develop . --command bash scripts/ci-gate.sh fixtures' ]]
[[ $CI_LOG_DIR == "$PWD/.git/fixture-logs" ]]
mkdir -p "$CI_LOG_DIR"
printf 'fixture log\n' > "$CI_LOG_DIR/fixtures.log"
NIX
    chmod +x "$scratch/bin/git" "$scratch/bin/nix"
    : > "$scratch/.core/unemupc/.git/test-status"
    PATH="$scratch/bin:$PATH" bash "$scratch/scripts/core-checkout.sh" --print-root > "$scratch/root"
    [[ $(cat "$scratch/root") == "UNEMUPC_FIXTURE_ROOT=$scratch/.core/unemupc" ]] || fail "self-test: clean print-root failed"
    for case_name in tracked untracked; do
        if [[ $case_name == tracked ]]; then
            printf ' M tracked\n' > "$scratch/.core/unemupc/.git/test-status"
        else
            printf '?? untracked\n' > "$scratch/.core/unemupc/.git/test-status"
        fi
        if PATH="$scratch/bin:$PATH" bash "$scratch/scripts/core-checkout.sh" --print-root > /dev/null 2> "$scratch/error"; then
            fail "self-test: print-root accepted $case_name changes"
        fi
        [[ -s $scratch/error ]] || fail "self-test: dirty print-root failed without an explanation"
    done
    : > "$scratch/.core/unemupc/.git/test-status"
    if ! PATH="$scratch/bin:$PATH" CI_LOG_DIR="$scratch/outside" bash "$scratch/scripts/core-checkout.sh" > /dev/null; then
        fail "self-test: fixture logs escaped the checkout"
    fi
    [[ -f $scratch/.core/unemupc/.git/fixture-logs/fixtures.log && ! -e $scratch/outside ]] || fail "self-test: unexpected fixture log location"
    echo "core-checkout: self-test passed (one valid lock; eleven invalid locks; clean and dirty print-root; contained fixture logs)"
)

mode=build
if [[ $# -gt 1 ]]; then
    fail "use one option; see --help"
fi
case ${1:-} in
    "") ;;
    --no-build) mode=checkout ;;
    --print-root) mode=print ;;
    --self-test) self_test; exit 0 ;;
    --help)
        cat <<'HELP'
Usage: bash scripts/core-checkout.sh [OPTION]
Check out the core revision from Cargo.lock and build its fixtures in its devShell.
  --no-build    Check out the revision without building fixtures.
  --print-root  Print the root only if the checkout is clean and has that revision.
  --self-test   Test lock, checkout and log guards without cloning or building.
  --help        Show this help.
UNEMUPC_CORE_REMOTE overrides the clone/fetch source.
The last stdout line is UNEMUPC_FIXTURE_ROOT=<absolute checkout path>.
HELP
        exit 0 ;;
    *) fail "unknown option: $1; see --help" ;;
esac

root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)
revision=$(lock_revision "$root/Cargo.lock")
checkout="$root/.core/unemupc"
remote=${UNEMUPC_CORE_REMOTE-https://github.com/unemu-org/unemupc}

# A symlink or linked worktree could send writes outside this private clone.
[[ ! -L $root/.core && ! -L $checkout ]] || fail "refusing a symlink at .core or .core/unemupc"
if [[ -e $checkout ]]; then
    [[ -d $checkout/.git && ! -L $checkout/.git ]] || fail ".core/unemupc must be a standalone git clone"
    [[ $(git -C "$checkout" rev-parse --show-toplevel) == "$checkout" ]] || fail "unexpected checkout root"
elif [[ $mode == print ]]; then
    fail ".core/unemupc is absent; run this script without --print-root first"
else
    mkdir -p -- "$root/.core"
    git clone --no-hardlinks -- "$remote" "$checkout" >&2
fi

[[ -z $(git -C "$checkout" status --porcelain --untracked-files=all) ]] || fail "refusing to touch dirty checkout .core/unemupc"
if [[ $mode == print ]]; then
    [[ $(git -C "$checkout" rev-parse HEAD) == "$revision" ]] || fail ".core/unemupc is not at the Cargo.lock revision $revision"
else
    if ! git -C "$checkout" cat-file -e "$revision^{commit}" 2>/dev/null; then
        git -C "$checkout" fetch -- "$remote" "$revision" >&2
    fi
    git -C "$checkout" checkout --detach "$revision" >&2
    if [[ $mode == build ]]; then
        # Gate logs belong to the clone; an inherited CI_LOG_DIR may point elsewhere.
        (cd -- "$checkout" && CI_LOG_DIR="$checkout/.git/fixture-logs" nix develop . --command bash scripts/ci-gate.sh fixtures) >&2
    fi
fi
printf 'UNEMUPC_FIXTURE_ROOT=%s\n' "$checkout"
