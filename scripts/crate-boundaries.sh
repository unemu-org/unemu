#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-or-later
# Keep neutral libraries, PC providers and application composition apart.
# Dependencies from github.com/unemu-org/unemupc count as PC dependencies,
# including optional, target-specific, build and test dependencies.
# unemu-host packages are neutral, including local source patches; audit
# their edges too so moving a library cannot hide a PC dependency.
# The walk follows normal/build edges after the starting crate's own edges.
# Window and CLI families stay separate except for the listed composition edge.
# Source includes must stay inside this repository, and the resolved graph
# must contain exactly one unemu-core-api package.
#
# Usage: scripts/crate-boundaries.sh [--self-test |
#          --metadata FILE [--allow FILE] | --resolved FILE]
# Needs cargo, jq and python3. AFFECTED_CARGO can wrap cargo.
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
default_allow="$root/scripts/crate-boundaries.allow"
fixtures="$root/scripts/crate-boundaries-fixtures"
# The crate every core and front end must link exactly once.
contract_crate=unemu-core-api

# Prints {"errors": [...], "packages": N, "allowed": N} for one metadata
# document and one allow list.
# Quoted assignments avoid Bash pipe deadlocks while loading these programs.
program='
def layers: ["contract", "neutral", "pc", "application"];
def may_use: {
  contract: [],
  neutral: ["contract", "neutral"],
  pc: ["contract", "neutral", "pc"],
  application: layers
};
def known_layer: . as $l | any(layers[]; . == $l);
def article: if test("^[aeiou]") then "an \(.)" else "a \(.)" end;
def family:
  if . == "cli" or . == "upcli" or startswith("cli-") then "CLI"
  elif . == "window" or . == "unemu" or . == "ui-kit" or . == "machine-view"
       or . == "library-model" or . == "library-ui" or . == "pc-media"
       or . == "pc-library" or . == "media-ui" or . == "pc-ui"
       or . == "player-ui" or startswith("window-") then "window"
  else null end;

# The rule $from breaks by depending on $to, if any. A crate without a known
# layer is reported on its own, not as a dependency.
def broken_rule($layer; $name; $from; $to):
  $layer[$from] as $fl | $layer[$to] as $tl
  | if $tl == null or ($tl | known_layer | not) then empty
    elif any(may_use | .[$fl][]; . == $tl) | not then
      "\($fl | article) crate may depend only on \(may_use | .[$fl] | join(", "))"
    elif ($name[$from] | family) != null and ($name[$to] | family) != null
         and ($name[$from] | family) != ($name[$to] | family) then
      "no \($name[$from] | family) crate depends on \($name[$to] | family | article) crate"
    else empty end;

def listed($valid; $from; $to; $kind):
  any($valid[]; .from == $from and .to == $to and .kind == $kind);

# The forbidden crates $from reaches, each once, by the shortest path, with
# the kind of the starting dependency of $from (normal before
# build before dev). The walk follows every dependency of $from and
# then the normal and build dependencies of the crates it reaches: a
# transitive dependency never builds its own test dependencies. It stops at
# a forbidden crate and never crosses an allow-listed edge.
def walk($graph; $layer; $name; $valid; $from):
  ([("normal", "build", "dev") as $k | ($graph[$from] // [])[] | select(.kind == $k)]
   | map(select(listed($valid; $from; .to; .kind) | not) | {path: [$from, .to], kind: .kind})) as $first
  | {queue: $first, seen: [$from], out: []}
  | until(.queue == [];
      .queue[0] as $item
      | .queue |= .[1:]
      | $item.path[-1] as $node
      | if any(.seen[]; . == $node) then .
        else .seen += [$node]
        | ([broken_rule($layer; $name; $from; $node)] | first // null) as $rule
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
| . as $declared
| (.resolved // .) as $resolved
| ($declared.workspace_members // []) as $workspace
| [($declared.packages // [])[]
   | select($workspace == [] or ((.id // .name) as $id | any($workspace[]; . == $id)))] as $packages
| ($resolved.packages // []) as $all
| def identity: .id // .name;
  def pc_source: (. // "" | test("^git[+]([a-z][a-z0-9+.-]*://)?([^/@]+@)?github[.]com[:/]unemu-org/unemupc([.]git)?([/?#]|$)"; "i"));
  def host_source: (. // "" | test("^git[+]([a-z][a-z0-9+.-]*://)?([^/@]+@)?github[.]com[:/]unemu-org/unemu-host([.]git)?([/?#]|$)"; "i"));
  def outside($root): $root != null and . != $root and (startswith($root + "/") | not);
  ($packages | map(identity)) as $members
| (($resolved.resolve.nodes // []) | map({key: .id, value: .deps}) | from_entries) as $nodes
# Declaration sources identify a local host patch even though its resolved
# package has no git source. Do not trust arbitrary external layer metadata.
| ([$all[] | select((.source | host_source) or (.id | host_source)) | identity]
   + [$all[] | . as $p | .dependencies[]? | select(.source | host_source) as $d
      | ($nodes[$p | identity] // [])[]
      | select(.name == (($d.rename // $d.name) | gsub("-"; "_"))
          and any(.dep_kinds[]?; (.kind // "normal") == ($d.kind // "normal"))) | .pkg]
   | unique) as $host_ids
| ($packages + [$all[] | identity as $id
    | select(($host_ids | index($id)) != null and ($members | index($id)) == null)]) as $audited
| ($all | map({key: identity, value: .name}) | from_entries) as $local_names
| [$all[].dependencies[]? | select(.source | pc_source)
   | {id: (.source + "#" + .name), name: .name}] as $declared_core
| ($local_names + ($declared_core | map({key: .id, value: .name}) | from_entries)) as $name
| (($all | map(. as $p | identity as $id | {key: $id, value:
      (if any($members[]; . == $id) then (.metadata.unemu.layer? // null)
       elif (.source | pc_source) or (.id | pc_source) then "pc"
       elif ($host_ids | index($id)) != null then "neutral"
       elif .source == null and (.manifest_path // "" | outside($declared.workspace_root)) then "pc"
       else null end)}) | from_entries)
   + ($declared_core | map({key: .id, value: "pc"}) | from_entries)) as $layer
# Resolved IDs preserve renames and source patches. Declaration edges retain
# optional/target dependencies absent from the active resolution.
| ($all | map(. as $p | {key: identity, value:
    ([($nodes[$p | identity] // [])[]
      | . as $d | .dep_kinds[]? | {to: $d.pkg, kind: (.kind // "normal")}]
     + [.dependencies[]? | . as $d
        | select(any(($nodes[$p | identity] // [])[];
            .name == (($d.rename // $d.name) | gsub("-"; "_"))
            and any(.dep_kinds[]?; (.kind // "normal") == ($d.kind // "normal"))) | not)
        | ([$all[] | select(.name == $d.name)
            | select(if $d.path != null then .manifest_path == ($d.path + "/Cargo.toml")
                     else .source == $d.source end) | identity] | first) as $target
        | if $target != null then {to: $target, kind: (.kind // "normal")}
          elif (.source | pc_source) then {to: (.source + "#" + .name), kind: (.kind // "normal")}
          else empty end] | unique)}) | from_entries) as $graph
| ($packages | map({key: .name, value: identity}) | from_entries) as $member_id
| ($valid | map(.from = $member_id[.from] | .to = $member_id[.to])) as $walk_valid
# An entry stands for one declared dependency that breaks a rule of its own
# crate; anything else is stale.
| ($valid | map(. as $e
    | . + {live: (any(($graph[$member_id[$e.from]] // [])[]; .to == $member_id[$e.to] and .kind == $e.kind)
                  and ([broken_rule($layer; $name; $member_id[$e.from]; $member_id[$e.to])] | length > 0))})) as $checked
| [ $audited[] | identity as $from | $layer[$from] as $fl
    | select($fl != null and $fl != "contract" and ($fl | known_layer))
    | walk($graph; $layer; $name; $walk_valid; $from) as $hit
    | $hit.path[-1] as $to
    | "forbidden: \($name[$from]) (\($fl)) → \($name[$to]) (\($layer[$to])), \($hit.kind) dependency, through \($hit.path | map($name[.]) | join(" → ")); rule: \($hit.rule)"
  ] as $forbidden
| {
    packages: ($packages | length),
    host_packages: ($audited | length) - ($packages | length),
    allowed: ([$checked[] | select(.live)] | length),
    errors: (
      (if $packages == [] then ["no packages: the metadata lists no workspace package"] else [] end)
      + [ $packages[] | select($layer[identity] == null)
        | "no layer: \(.name) has no [package.metadata.unemu] layer; set one of \(layers | join(", "))" ]
      + [ $packages[] | select($layer[identity] != null and ($layer[identity] | known_layer | not))
        | "unknown layer: \(.name) declares layer \($layer[identity] | tojson); use one of \(layers | join(", "))" ]
      + [ $packages[] | select($layer[identity] == "contract") | .name as $from | .dependencies[]?
        | "forbidden: \($from) (contract) depends on \(.name) (\(.kind // "normal") dependency); rule: a contract crate depends on nothing" ]
      + [ $audited[] | select(.name == "unemu-test-core") | .name as $from | .dependencies[]?
        | select(.name != "unemu-core-api")
        | "forbidden: \($from) depends on \(.name) (\(.kind // "normal") dependency); rule: the test core depends on the contract alone" ]
      + [ $audited[] | identity as $id | .name as $from | .dependencies[]?
        | select(.path != null and (.path | outside($declared.workspace_root)))
        # Host crates can link one another by path in their upstream checkout.
        | .path as $path
        | select(($members | index($id)) != null
            or (any($all[]; identity as $target
                | .manifest_path == ($path + "/Cargo.toml")
                  and ($host_ids | index($target)) != null) | not))
        | "outside repository: \($from) declares path dependency \(.name); use a local [patch] override instead" ]
      + $forbidden
      + [ $checked[] | select(.live | not)
          | "stale allow entry, line \(.line): \(.text); that dependency is gone or no longer forbidden, so delete the line" ]
      + [ $entries[] | select(.malformed)
          | "malformed allow entry, line \(.line): \(.text); expected FROM → TO (normal|build|dev): REASON" ]
    )
  }
'

# Prints {"errors": [...], "source": S} for one resolved metadata document:
# an error unless exactly one package is named $crate.
one_copy_program='
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
'

# check_one_copy METADATA: print each error, or the contract's source; fail
# unless the graph holds exactly one copy.
check_one_copy() {
    local result
    result=$(jq -c --arg crate "$contract_crate" "$one_copy_program" "$1") || {
        echo "crate boundaries: cannot read the resolved metadata $1" >&2
        return 2
    }
    printf '%s' "$result" | jq -r '.errors[]'
    if [[ $(printf '%s' "$result" | jq '.errors | length') -ne 0 ]]; then
        echo "crate boundaries: not exactly one copy of $contract_crate" >&2
        return 1
    fi
    printf '%s' "$result" | jq -r --arg crate "$contract_crate" '"crate boundaries: one copy of \($crate), from \(.source)"'
}

# check METADATA ALLOW: print each error and a summary; fail on any error.
check() {
    local result count
    result=$(jq -c --rawfile allow_text "$2" "$program" "$1") || {
        echo "crate boundaries: cannot read $1 with the allow list $2" >&2
        return 2
    }
    printf '%s' "$result" | jq -r '.errors[]'
    count=$(printf '%s' "$result" | jq '.errors | length')
    if [[ $count -ne 0 ]]; then
        echo "crate boundaries: $count error(s)" >&2
        return 1
    fi
    printf '%s' "$result" | jq -r '"crate boundaries: \(.packages) workspace crates and \(.host_packages) host crates checked, \(.allowed) allowed exception(s)"'
}

# Keep Python in a file: large here-documents can deadlock Bash pipes.
check_sources() {
    python3 "$root/scripts/crate-boundaries.py" "$@"
}

# Each fixture is metadata with one planted case and its expectation:
# "self_test": {"expect": "pass"|"fail", "message": "text the output contains",
# "errors": N (optional, how many errors it reports)}, and an optional allow
# list NAME.allow beside it.
self_test() {
    local fixture name allow list expect message errors output status failed=0
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
    check_sources --self-test || failed=1
    return "$failed"
}

case "${1:-}" in
    "")
        metadata=$(mktemp)
        resolved=$(mktemp)
        trap 'rm -f "$metadata" "$resolved"' EXIT
        read -r -a cargo_cmd <<< "${AFFECTED_CARGO:-cargo}"
        (cd "$root" && "${cargo_cmd[@]}" metadata --offline --format-version 1 --no-deps) >"$metadata"
        (cd "$root" && "${cargo_cmd[@]}" metadata --format-version 1 --offline --locked --all-features) >"$resolved"
        status=0
        combined=$(mktemp)
        jq --slurpfile resolved "$resolved" '. + {resolved: $resolved[0]}' "$metadata" >"$combined"
        check "$combined" "$default_allow" || status=$?
        rm -f "$combined"
        check_sources "$metadata" || { s=$?; [[ $status -ne 0 ]] || status=$s; }
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
