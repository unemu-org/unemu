#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-or-later
# Keep the workspace's crate layers apart, so the front end, the platform
# providers and the emulator cores can later live in separate repositories
# (backlog/docs/design/core-contract.md §7.3 and §7.4).
#
# The check reads Cargo dependencies only: a test that includes another
# crate's source file with #[path] (the shared golden support under
# crates/pc-harness/tests/support) is invisible to it, and only the compiler
# stops such a file from naming a crate its includer may not use.
#
# Every workspace package names its layer in its manifest:
#
#   [package.metadata.unemu]
#   layer = "neutral"   # contract | neutral | emulation | pc | application
#
# A crate may depend on these layers, through normal, build and test
# dependencies alike, directly or through other workspace crates:
#
#   contract     nothing at all, also outside the workspace
#   neutral      contract, neutral
#   emulation    contract, emulation
#   pc           contract, neutral, emulation, pc
#   application  anything
#
# and no window crate (window, window-*, ui-kit, machine-view, library-model,
# library-ui, pc-media, pc-library, media-ui, pc-ui, unemupc) depends on a CLI
# crate (cli, cli-*, upcli), nor the other way round.
#
# The test core, unemu-test-core, is what the front end's crates test against
# instead of a real platform, so it declares no dependency of any kind but the
# contract, unemu-core-api, not even one from outside the workspace.
#
# The contract crate, unemu-core-api, lives in its own repository and comes in
# as a pinned git dependency, so no workspace package has the contract layer
# any more: the rule that a contract crate depends on nothing is checked by the
# contract's own CI, and here it only applies if such a package comes back.
#
# One copy of the contract. Two revisions of unemu-core-api in one build are
# two different crates, and a core built against one does not implement the
# other's traits; the compiler then fails with a long error far from the
# cause. So the resolved dependency graph (`cargo metadata --offline --locked`,
# every package the lock file pins) must hold exactly one package named
# unemu-core-api; otherwise the check prints each copy's source and the
# packages that depend on it.
#
# The check reads the dependencies each manifest declares (optional and
# target-specific ones included), not a resolved build. From each crate it
# follows the crate's own dependencies of every kind, then the normal and build
# dependencies of the crates it reaches, and reports the first forbidden crate
# on each path once, with the path.
#
# Known violations are listed in scripts/crate-boundaries.allow, one declared
# dependency per line: `FROM → TO (KIND): REASON`, KIND being normal, build or
# dev. The walk never crosses a listed dependency, so a crate that reaches it
# through FROM needs no entry of its own. A listed dependency that is gone or
# no longer forbidden fails, so the list only shrinks.
#
# Usage:
#   scripts/crate-boundaries.sh                  check the workspace
#   scripts/crate-boundaries.sh --self-test      run the rules over the fixtures
#   scripts/crate-boundaries.sh --metadata FILE [--allow FILE]
#                                                check `cargo metadata --no-deps` output
#   scripts/crate-boundaries.sh --resolved FILE  check one copy of the contract in
#                                                full `cargo metadata` output
#
# Needs cargo and jq (both in the devShell). The workspace check reads one
# `cargo metadata --no-deps`, which resolves nothing, and one resolved
# `cargo metadata --offline --locked`, which reads the lock file and the local
# dependency cache; both take well under a second.
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
default_allow="$root/scripts/crate-boundaries.allow"
fixtures="$root/scripts/crate-boundaries-fixtures"
# The crate every core and front end must link exactly once.
contract_crate=unemu-core-api

# Prints {"errors": [...], "packages": N, "allowed": N} for one metadata
# document and one allow list.
read -r -d '' program <<'JQ' || true
def layers: ["contract", "neutral", "emulation", "pc", "application"];
def may_use: {
  contract: [],
  neutral: ["contract", "neutral"],
  emulation: ["contract", "emulation"],
  pc: ["contract", "neutral", "emulation", "pc"],
  application: layers
};
def known_layer: . as $l | any(layers[]; . == $l);
def article: if test("^[aeiou]") then "an \(.)" else "a \(.)" end;
def family:
  if . == "cli" or . == "upcli" or startswith("cli-") then "CLI"
  elif . == "window" or . == "unemupc" or . == "ui-kit" or . == "machine-view"
       or . == "library-model" or . == "library-ui" or . == "pc-media"
       or . == "pc-library" or . == "media-ui" or . == "pc-ui"
       or startswith("window-") then "window"
  else null end;

# The rule $from breaks by depending on $to, if any. A crate without a known
# layer is reported on its own, not as a dependency.
def broken_rule($layer; $from; $to):
  $layer[$from] as $fl | $layer[$to] as $tl
  | if $tl == null or ($tl | known_layer | not) then empty
    elif any(may_use | .[$fl][]; . == $tl) | not then
      "\($fl | article) crate may depend only on \(may_use | .[$fl] | join(", "))"
    elif ($from | family) != null and ($to | family) != null
         and ($from | family) != ($to | family) then
      "no \($from | family) crate depends on \($to | family | article) crate"
    else empty end;

def listed($valid; $from; $to; $kind):
  any($valid[]; .from == $from and .to == $to and .kind == $kind);

# The forbidden crates $from reaches, each once, by the shortest path, with
# the kind of $from's own dependency the path starts with (normal before
# build before dev). The walk follows $from's dependencies of every kind and
# then the normal and build dependencies of the crates it reaches: a
# dependency's own test dependencies are never built for $from. It stops at
# a forbidden crate and never crosses an allow-listed edge.
def walk($graph; $layer; $valid; $from):
  ([("normal", "build", "dev") as $k | ($graph[$from] // [])[] | select(.kind == $k)]
   | map(select(listed($valid; $from; .to; .kind) | not) | {path: [$from, .to], kind: .kind})) as $first
  | {queue: $first, seen: [$from], out: []}
  | until(.queue == [];
      .queue[0] as $item
      | .queue |= .[1:]
      | $item.path[-1] as $node
      | if any(.seen[]; . == $node) then .
        else .seen += [$node]
        | ([broken_rule($layer; $from; $node)] | first // null) as $rule
        | if $rule != null then .out += [$item + {rule: $rule}]
          else .queue += [($graph[$node] // [])[] | select(.kind != "dev")
                          | select(listed($valid; $node; .to; .kind) | not)
                          | {path: ($item.path + [.to]), kind: $item.kind}]
          end
        end)
  | .out[];

($allow_text | split("\n") | to_entries
  | map(select(.value | test("^\\s*(#|$)") | not))
  | map(. as $line
      | (($line.value
          | capture("^(?<from>[^ ]+) → (?<to>[^ ]+) \\((?<kind>normal|build|dev)\\): (?<reason>[^ ].*)$"))
         // {malformed: true})
      | . + {line: ($line.key + 1), text: $line.value})) as $entries
| ($entries | map(select(.malformed | not))) as $valid
| (.packages // []) as $packages
| ($packages | map(.name)) as $names
| ($packages | map({key: .name, value: (.metadata.unemu.layer? // null)}) | from_entries) as $layer
| ($packages
   | map({key: .name, value: ([.dependencies[]?
       | select(.source == null)
       | .name as $n | select(any($names[]; . == $n))
       | {to: .name, kind: (.kind // "normal")}] | unique)})
   | from_entries) as $graph
# An entry stands for one declared dependency that breaks a rule of its own
# crate; anything else is stale.
| ($valid | map(. as $e
    | . + {live: (any(($graph[$e.from] // [])[]; .to == $e.to and .kind == $e.kind)
                  and ([broken_rule($layer; $e.from; $e.to)] | length > 0))})) as $checked
| [ $packages[] | .name as $from | $layer[$from] as $fl
    | select($fl != null and $fl != "contract" and ($fl | known_layer))
    | walk($graph; $layer; $valid; $from) as $hit
    | $hit.path[-1] as $to
    | "forbidden: \($from) (\($fl)) → \($to) (\($layer[$to])), \($hit.kind) dependency, through \($hit.path | join(" → ")); rule: \($hit.rule)"
  ] as $forbidden
| {
    packages: ($packages | length),
    allowed: ([$checked[] | select(.live)] | length),
    errors: (
      (if $packages == [] then ["no packages: the metadata lists no workspace package"] else [] end)
      + [ $packages[] | select($layer[.name] == null)
        | "no layer: \(.name) has no [package.metadata.unemu] layer; set one of \(layers | join(", "))" ]
      + [ $packages[] | select($layer[.name] != null and ($layer[.name] | known_layer | not))
        | "unknown layer: \(.name) declares layer \($layer[.name] | tojson); use one of \(layers | join(", "))" ]
      + [ $packages[] | select($layer[.name] == "contract") | .name as $from | .dependencies[]?
        | "forbidden: \($from) (contract) depends on \(.name) (\(.kind // "normal") dependency); rule: a contract crate depends on nothing" ]
      + [ $packages[] | select(.name == "unemu-test-core") | .name as $from | .dependencies[]?
        | select(.name != "unemu-core-api")
        | "forbidden: \($from) depends on \(.name) (\(.kind // "normal") dependency); rule: the test core depends on the contract alone" ]
      + $forbidden
      + [ $checked[] | select(.live | not)
          | "stale allow entry, line \(.line): \(.text); that dependency is gone or no longer forbidden, so delete the line" ]
      + [ $entries[] | select(.malformed)
          | "malformed allow entry, line \(.line): \(.text); expected FROM → TO (normal|build|dev): REASON" ]
    )
  }
JQ

# Prints {"errors": [...], "source": S} for one resolved metadata document:
# an error unless exactly one package is named $crate.
read -r -d '' one_copy_program <<'JQ' || true
(.packages // []) as $packages
| ($packages | map({key: .id, value: .name}) | from_entries) as $name_of
| [$packages[] | select(.name == $crate)] as $copies
| def where: .source // "path \(.manifest_path | rtrimstr("/Cargo.toml"))";
  def users($id):
    [(.resolve.nodes // [])[] | select(any(.deps[]?; .pkg == $id)) | $name_of[.id]]
    | unique | if . == [] then "no package" else join(", ") end;
  . as $doc
| {
    source: (if ($copies | length) == 1 then ($copies[0] | where) else null end),
    errors: (
      if ($copies | length) == 1 then []
      elif $copies == [] then
        ["one copy: the resolved graph has no package named \($crate)"]
      else
        ["one copy: \($copies | length) copies of \($crate) in the resolved graph; every core and front end must link the same one"]
        + [$copies[] | "    \(.version) from \(where), used by \(.id as $id | $doc | users($id))"]
      end)
  }
JQ

# check_one_copy METADATA: print each error, or the contract's source; fail
# unless the graph holds exactly one copy.
check_one_copy() {
    local result
    result=$(jq -c --arg crate "$contract_crate" "$one_copy_program" "$1") || {
        echo "crate boundaries: cannot read the resolved metadata $1" >&2
        return 2
    }
    jq -r '.errors[]' <<<"$result"
    if [[ $(jq '.errors | length' <<<"$result") -ne 0 ]]; then
        echo "crate boundaries: not exactly one copy of $contract_crate" >&2
        return 1
    fi
    jq -r --arg crate "$contract_crate" '"crate boundaries: one copy of \($crate), from \(.source)"' <<<"$result"
}

# check METADATA ALLOW: print each error and a summary; fail on any error.
check() {
    local result count
    result=$(jq -c --rawfile allow_text "$2" "$program" "$1") || {
        echo "crate boundaries: cannot read $1 with the allow list $2" >&2
        return 2
    }
    jq -r '.errors[]' <<<"$result"
    count=$(jq '.errors | length' <<<"$result")
    if [[ $count -ne 0 ]]; then
        echo "crate boundaries: $count error(s)" >&2
        return 1
    fi
    jq -r '"crate boundaries: \(.packages) crates checked, \(.allowed) allowed exception(s)"' <<<"$result"
}

# Each fixture is metadata with one planted case and its expectation:
# "self_test": {"expect": "pass"|"fail", "message": "text the output contains",
# "errors": N (optional, how many errors it reports)}, and an optional allow
# list NAME.allow beside it.
self_test() {
    local fixture name allow expect message errors output status failed=0
    for fixture in "$fixtures"/*.json; do
        name=$(basename "$fixture" .json)
        allow="$fixtures/$name.allow"
        [[ -f $allow ]] || allow=/dev/null
        expect=$(jq -r '.self_test.expect' "$fixture")
        message=$(jq -r '.self_test.message // ""' "$fixture")
        errors=$(jq -r '.self_test.errors // ""' "$fixture")
        status=0
        output=$(check "$fixture" "$allow" 2>&1) || status=$?
        if [[ $expect == pass && $status -eq 0 ]] ||
            [[ $expect == fail && $status -ne 0 && $output == *"$message"* &&
                ( -z $errors || $output == *"crate boundaries: $errors error(s)"* ) ]]; then
            echo "self-test $name: ${expect}ed as expected"
        else
            echo "self-test $name: expected $expect${message:+ with \"$message\"}${errors:+ and $errors error(s)}, got status $status:" >&2
            printf '%s\n' "$output" | sed 's/^/    /' >&2
            failed=1
        fi
    done
    # Fixtures of resolved metadata for the one-copy rule, with the same
    # "self_test" expectation (without "errors").
    for fixture in "$fixtures"/resolved/*.json; do
        name=resolved/$(basename "$fixture" .json)
        expect=$(jq -r '.self_test.expect' "$fixture")
        message=$(jq -r '.self_test.message // ""' "$fixture")
        status=0
        output=$(check_one_copy "$fixture" 2>&1) || status=$?
        if [[ $expect == pass && $status -eq 0 ]] ||
            [[ $expect == fail && $status -ne 0 && $output == *"$message"* ]]; then
            echo "self-test $name: ${expect}ed as expected"
        else
            echo "self-test $name: expected $expect${message:+ with \"$message\"}, got status $status:" >&2
            printf '%s\n' "$output" | sed 's/^/    /' >&2
            failed=1
        fi
    done
    return "$failed"
}

case "${1:-}" in
    "")
        metadata=$(mktemp)
        resolved=$(mktemp)
        trap 'rm -f "$metadata" "$resolved"' EXIT
        (cd "$root" && cargo metadata --format-version 1 --no-deps) >"$metadata"
        (cd "$root" && cargo metadata --format-version 1 --offline --locked) >"$resolved"
        status=0
        check "$metadata" "$default_allow" || status=$?
        check_one_copy "$resolved" || { s=$?; [[ $status -ne 0 ]] || status=$s; }
        exit "$status"
        ;;
    --self-test)
        [[ $# -eq 1 ]] || { echo "usage: $0 --self-test" >&2; exit 2; }
        self_test
        ;;
    --metadata)
        [[ $# -eq 2 || ($# -eq 4 && $3 == --allow) ]] ||
            { echo "usage: $0 --metadata FILE [--allow FILE]" >&2; exit 2; }
        check "$2" "${4:-/dev/null}"
        ;;
    --resolved)
        [[ $# -eq 2 ]] || { echo "usage: $0 --resolved FILE" >&2; exit 2; }
        check_one_copy "$2"
        ;;
    *)
        echo "usage: $0 [--self-test | --metadata FILE [--allow FILE] | --resolved FILE]" >&2
        exit 2
        ;;
esac
