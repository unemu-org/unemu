#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-or-later
# Keep host access visible until it leaves the core (core-contract.md §12.2).
# This is a spelling check, not Rust name resolution: grouped imports and
# common short paths count too. Comments and separate test sources are skipped;
# each src file stops at its first real #[cfg(test)] attribute, as specified
# in §12.3.
# Exact source lines keep an exception from silently covering a new use.
#
# Usage: bash scripts/core-host-guard.sh [--allow FILE] [--metadata FILE] [--root DIR]
#        bash scripts/core-host-guard.sh --self-test
# Metadata is cargo metadata --no-deps output, with the same layer tags the
# crate-boundary check reads. --root and --metadata also let fixtures use the
# real scanner without changing the workspace. Needs cargo and jq.
set -euo pipefail
export LC_ALL=C
root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
allow="$root/scripts/core-host-guard.allow"
metadata=
self_test=0
while (( $# )); do
    case "$1" in
        --allow|--metadata|--root)
            [[ $# -ge 2 ]] || exit 2
            case "$1" in
                --allow) allow=$2 ;;
                --metadata) metadata=$2 ;;
                --root) root=$2 ;;
            esac
            shift 2 ;;
        --self-test) self_test=1; shift ;;
        *) echo "core host guard: unknown argument $1" >&2; exit 2 ;;
    esac
done
scratch=$(mktemp -d "${TMPDIR:-/tmp}/core-host-guard.XXXXXX")
trap 'rm -rf "$scratch"' EXIT

scan() {
    local manifest layer source file
    : > "$scratch/hits"
    jq -er '.packages | length > 0' "$metadata" >/dev/null
    jq -r '.packages[] | select(.metadata.unemu.layer == "contract" or .metadata.unemu.layer == "emulation")
        | [.manifest_path, .metadata.unemu.layer] | @tsv' "$metadata" > "$scratch/crates"
    while IFS=$'\t' read -r manifest layer; do
        source="${manifest%/*}/src"
        while IFS= read -r file; do
            case "$file" in */tests/*|*/tests.rs|*_tests.rs) continue ;; esac
            awk -v file="${file#"$root"/}" -v layer="$layer" '
                # Comments cannot hide production code or count as host access.
                function uncomment(line, out, i, pair, c, quoted, escaped) {
                    for (i = 1; i <= length(line); i++) {
                        pair = substr(line, i, 2)
                        c = substr(line, i, 1)
                        if (block_depth) {
                            if (pair == "/*") { block_depth++; i++ }
                            else if (pair == "*/") { block_depth--; i++; out = out " " }
                        } else if (quoted) {
                            out = out c
                            if (escaped) escaped = 0
                            else if (c == "\\") escaped = 1
                            else if (c == "\"") quoted = 0
                        } else if (pair == "//") break
                        else if (pair == "/*") { block_depth++; i++; out = out " " }
                        else {
                            out = out c
                            if (c == "\"") quoted = 1
                        }
                    }
                    return out
                }
                {
                    code = uncomment($0)
                    if (code ~ /^[[:space:]]*#\[cfg\(test\)\]/) exit
                    if (code ~ /^[[:space:]]*$/) next
                    text = $0
                    sub(/^[[:space:]]+/, "", text)
                    sub(/[[:space:]]+$/, "", text)
                    host = "std[[:space:]]*::[[:space:]]*\\{[^;]*(fs|env|net|process|thread|time|os)([^[:alnum:]_]|$)|(^|[^[:alnum:]_])(std[[:space:]]*::[[:space:]]*)?(fs|env|net|process|thread|time|os)[[:space:]]*(::|;|as[[:space:]])"
                    grouped = "^[[:space:]]*(fs|env|net|process|thread|time|os)[[:space:]]*(,|::|\\{|as[[:space:]]|$)"
                    names = "(^|[^[:alnum:]_])(Instant|SystemTime|OnceLock|LazyLock|HashMap|RandomState|stdin|stdout|stderr)([^[:alnum:]_]|$)"
                    calls = "(^|[^[:alnum:]_])(println|eprintln|print|eprint|dbg|thread_local)[[:space:]]*!|io[[:space:]]*::[[:space:]]*(stdin|stdout|stderr)|(^|[^[:alnum:]_])(File|OpenOptions)[[:space:]]*::"
                    state = "static[[:space:]]+mut|extern[[:space:]]+\"(C|system)\"|cfg[[:space:]]*[!(].*(unix|windows|target_os)|(^|[^[:alnum:]_])(rand|rand_core|getrandom)[[:space:]]*::"
                    forbidden = "(^|[^[:alnum:]_])unsafe([^[:alnum:]_]|$)"
                    libc = "(^|[^[:alnum:]_])libc[[:space:]]*::|extern[[:space:]]+crate[[:space:]]+libc([^[:alnum:]_]|$)"
                    path = "(^|[^[:alnum:]_])(std[[:space:]]*::[[:space:]]*)?path[[:space:]]*::|^[[:space:]]*path[[:space:]]*[,:{]"
                    if (code ~ host || code ~ grouped || code ~ names || code ~ calls || code ~ state || code ~ forbidden || (layer == "emulation" && code ~ libc) || (layer == "contract" && code ~ path))
                        print file "\t" FNR "\t" text
                }
            ' "$file" >> "$scratch/hits" || return 2
        done < <(find "$source" -type f -name '*.rs' | sort)
    done < "$scratch/crates"
    jq -Rnr --rawfile allow "$allow" '
        [inputs | split("\t") | {file: .[0], line: .[1], text: .[2]}] as $hits
        | ($allow | split("\n") | to_entries
            | map(select(.value | test("^\\s*(#|$)") | not))
            | map(. as $line | ((.value | capture("^(?<file>[^:]+): (?<text>.*): (?<reason>[^:]+)$"))
                // {malformed: true}) + {line: ($line.key + 1), entry: $line.value})) as $entries
        | [$entries[] | select(.malformed | not)] as $valid
        | [ ($hits[] as $hit | select(any($valid[]; .file == $hit.file and .text == $hit.text) | not)
                | "unlisted: \($hit.file):\($hit.line): \($hit.text)"),
            ($entries[] | select(.malformed) | "malformed allow entry, line \(.line): \(.entry)"),
            ($valid[] as $entry | select(any($hits[]; .file == $entry.file and .text == $entry.text) | not)
                | "stale allow entry, line \($entry.line): \($entry.entry)"),
            ($valid | group_by([.file, .text])[] | select(length > 1)
                | "duplicate allow entry: \(.[0].file): \(.[0].text)") ]
        | .[], "core host guard: \(length) error(s); \($hits | length) matched line(s), \($valid | length) allow entry/entries"
    ' < "$scratch/hits" > "$scratch/result" || return 2
    cat "$scratch/result"
    [[ $(tail -n 1 "$scratch/result") == 'core host guard: 0 error(s);'* ]]
}

if (( self_test )); then
    root="$scratch/tree"
    metadata="$scratch/metadata.json"
    allow="$scratch/allow"
    mkdir -p "$root/crates/devices/src" "$root/crates/device/src" "$root/crates/host/src"
    jq -n --arg root "$root" '{packages: ["devices", "device", "host"] | map({
        name: ., manifest_path: ($root + "/crates/" + . + "/Cargo.toml"),
        metadata: {unemu: {layer: (if . == "host" then "neutral" else "emulation" end)}}})}' > "$metadata"
    : > "$allow"
    printf 'std::time::Instant::now();\n' > "$root/crates/host/src/lib.rs"
    failures=0
    cases=0
    planted_cases=0
    case_check() {
        local name=$1 expected=$2 message=$3 status=0
        cases=$((cases + 1))
        scan > "$scratch/case.log" || status=$?
        if [[ $status == "$expected" ]] && grep -Fq "$message" "$scratch/case.log"; then
            echo "self-test $name: status $status as expected ($message)"
        else
            echo "self-test $name: expected status $expected with $message, got $status" >&2
            cat "$scratch/case.log" >&2
            failures=$((failures + 1))
        fi
    }
    case_check clean 0 '0 error(s)'
    printf 'fn guest(regs: Registers, time: u64) {\n    let _ = Registers { fs: regs.fs, };\n    consume(time, regs.fs);\n}\n' > "$root/crates/device/src/lib.rs"
    case_check guest-names 0 '0 error(s)'
    : > "$root/crates/device/src/lib.rs"
    for planted in 'std::time::Instant::now();' 'use libc::c_int;' 'extern crate libc;' 'pub fn bad() { unsafe {} }' \
        'use std::{fs, env};' 'use std::time::{Duration, Instant};' \
        'use std::io::{self, stdout};' 'use std::fs as f;' \
        'use std::io::{stdin}; fn host() { let _ = stdin(); }' \
        'use std::io::{stdout}; fn host() { let _ = stdout(); }' \
        'use std::io::{stderr}; fn host() { let _ = stderr(); }' \
        'use std::io::{stdin as input};' 'use std::io::{stdout as output};' \
        'use std::io::{stderr as errors};' 'use std::{fs as f, env as e};' \
        'use std::net as network;' 'use std::process as child;' \
        'use std::thread as threads;' 'use std::time as clock;' \
        'use std::env as environment;' 'use std::os as platform;' \
        'use std::net::TcpStream;' 'use std::thread;' \
        'std::process::exit(1);' 'std::io::stdout();' 'println!("host");' \
        'static mut SHARED: u8 = 0;' 'thread_local! {}' 'use std::sync::LazyLock;' \
        'extern "C" {}' '#[cfg(target_os = "linux")]' 'use std::os::unix;' \
        'getrandom::fill(&mut []);' 'use std::collections::HashMap;'; do
        case "$planted" in *Instant*) crate=devices ;; *) crate=device ;; esac
        printf '%s\n' "$planted" > "$root/crates/$crate/src/lib.rs"
        case_check "$planted" 1 "unlisted: crates/$crate/src/lib.rs:1: $planted"
        planted_cases=$((planted_cases + 1))
        : > "$root/crates/$crate/src/lib.rs"
    done
    for comment in '// Example: #[cfg(test)]' '/// Example: #[cfg(test)]' \
        '/* Example: #[cfg(test)] */' '/* Example:\n#[cfg(test)]\n*/'; do
        printf '%b\nstd::time::Instant::now();\n' "$comment" > "$root/crates/devices/src/lib.rs"
        case_check "comment-before-production: $comment" 1 'std::time::Instant::now();'
        planted_cases=$((planted_cases + 1))
    done
    for planted in '/* std::fs::read("x"); */ std::io::stdout();' \
        'std::io::stdout(); /* comment */' \
        'let _ = "/*"; std::io::stdout();' 'let _ = "//"; std::io::stdout();' \
        'let _ = "Example: #[cfg(test)]"; std::io::stdout();'; do
        printf '%s\n' "$planted" > "$root/crates/devices/src/lib.rs"
        case_check "production-with-comment-or-string: $planted" 1 "$planted"
        planted_cases=$((planted_cases + 1))
    done
    : > "$root/crates/devices/src/lib.rs"
    for comment in '// std::fs::read("x");' '/// std::fs::read("x");' \
        '//! std::fs::read("x");' '/* std::fs::read("x"); */' \
        '/*\nstd::fs::read("x");\n*/' \
        '/* outer /* nested */ std::fs::read("x"); */' \
        'fn guest() { /* std::fs::read("x"); */ }'; do
        printf '%b\n' "$comment" > "$root/crates/device/src/lib.rs"
        case_check "comment-excluded: $comment" 0 '0 error(s)'
    done
    : > "$root/crates/device/src/lib.rs"
    for excluded in src/tests.rs src/host_tests.rs src/tests/host.rs src/nested/tests/host.rs \
        tests/host.rs benches/host.rs examples/host.rs; do
        mkdir -p "$root/crates/device/$(dirname "$excluded")"
        printf 'std::time::Instant::now();\n' > "$root/crates/device/$excluded"
        case_check "separate-file-excluded: $excluded" 0 '0 error(s)'
        rm "$root/crates/device/$excluded"
    done
    printf 'std::env::var("HOME");\n' > "$root/crates/device/src/lib.rs"
    printf 'crates/device/src/lib.rs: std::env::var("HOME");: fixture\n' > "$allow"
    case_check listed 0 '0 error(s)'
    cat "$allow" >> "$scratch/duplicate"
    cat "$allow" >> "$scratch/duplicate"
    cp "$scratch/duplicate" "$allow"
    case_check duplicate-entry 1 'duplicate allow entry'
    printf 'not an allow entry\n' > "$allow"
    case_check malformed-entry 1 'malformed allow entry'
    : > "$allow"
    case_check removed-allow-entry 1 'unlisted: crates/device/src/lib.rs:1:'
    printf 'crates/device/src/lib.rs: std::env::var("HOME");: fixture\n' > "$allow"
    : > "$root/crates/device/src/lib.rs"
    case_check removed-use 1 'stale allow entry'
    printf 'std::env::var("HOME");\n' > "$root/crates/devices/src/lib.rs"
    case_check moved-use 1 'unlisted: crates/devices/src/lib.rs:1:'
    : > "$allow"
    printf '#[cfg(test)]\nmod tests { fn host() { std::time::Instant::now(); } }\n' > "$root/crates/devices/src/lib.rs"
    case_check tests-excluded 0 '0 error(s)'
    echo "core host guard self-test: $cases case(s), $planted_cases planted host case(s), $failures failure(s)"
    [[ $failures == 0 ]]
else
    if [[ -z $metadata ]]; then
        metadata="$scratch/metadata.json"
        read -r -a cargo_cmd <<< "${AFFECTED_CARGO:-cargo}"
        (cd "$root" && "${cargo_cmd[@]}" metadata --format-version 1 --offline --no-deps) > "$metadata"
    fi
    scan
fi
