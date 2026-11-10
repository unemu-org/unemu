// SPDX-License-Identifier: AGPL-3.0-or-later
//! YAML-specific hardening and scalar contracts from the machine-files design.

use machine_file::{
    parse_yaml,
    schema::v1::{CpuModel, NumberOrNone},
};
use std::path::Path;

fn rejected(input: &str, message: &str) {
    let error = parse_yaml("machine.yaml", input).unwrap_err();
    assert!(error.message().contains(message), "{error}");
    assert!(error.line_column().is_some(), "{error}");
}

#[test]
fn strict_booleans_reject_legacy_spellings_on_cards_and_drives() {
    for value in ["yes", "no", "on", "off", "YES", "NO", "On", "Off"] {
        rejected(&format!("schema: 1\ncard:\n  - remove: {value}\n"), "bool");
        rejected(
            &format!("schema: 1\ncard:\n  - drives:\n      - remove: {value}\n"),
            "bool",
        );
    }
    for (value, expected) in [("true", true), ("false", false)] {
        let doc = parse_yaml(
            "bool.yaml",
            &format!("schema: 1\ncard: [{{remove: {value}, drives: [{{remove: {value}}}]}}]\n"),
        )
        .unwrap();
        let cards = doc.card.unwrap();
        assert_eq!(cards[0].remove, Some(expected));
        assert_eq!(cards[0].drives.as_ref().unwrap()[0].remove, Some(expected));
    }
}

#[test]
fn duplicate_keys_are_errors_even_inside_ignored_peek_fields() {
    for input in [
        "schema: 1\nname: first\nname: second\n",
        "schema: 1\nboard: {model: ibm-5170, model: ibm-5170}\n",
        "schema: 1\ncard: [{kind: mda, id: first, id: second}]\n",
    ] {
        rejected(input, "duplicate");
    }
}

#[test]
fn exactly_one_document_is_required_even_if_the_second_is_empty() {
    for suffix in ["---\nschema: 1\n", "---\n", "...\n---\n", "---\nnull\n"] {
        rejected(&format!("schema: 1\n{suffix}"), "document");
    }
    for input in ["", "---\n", "null\n"] {
        assert!(parse_yaml("empty.yaml", input).is_err());
    }
    assert!(parse_yaml("one.yaml", "---\nschema: 1\n...\n").is_ok());
}

#[test]
fn anchor_count_is_bounded() {
    let doc = parse_yaml(
        "aliases.yaml",
        "schema: 1\nname: &name repeated\ndescription: *name\n",
    )
    .unwrap();
    assert_eq!(doc.name, doc.description);
    let mut input = "schema: 1\ncard:\n".to_owned();
    for index in 0..33 {
        input.push_str(&format!("  - &card{index} {{kind: mda}}\n"));
    }
    rejected(&input, "anchor");
}

#[test]
fn parser_event_depth_and_file_budgets_are_enforced() {
    let cards = "  - {kind: mda}\n".repeat(4000);
    rejected(&format!("schema: 1\ncard:\n{cards}"), "budget");
    let nested = format!("{}0{}", "[".repeat(40), "]".repeat(40));
    rejected(&format!("schema: 1\nfuture: {nested}\n"), "depth");
    let error = parse_yaml(
        "machine.yaml",
        &format!("schema: 1\nname: {}\n", "x".repeat(1_048_577)),
    )
    .unwrap_err();
    assert_eq!(
        error.message(),
        "machine file exceeds size limit of 1048576 bytes"
    );
    assert_eq!(error.path(), Path::new("machine.yaml"));
}

#[test]
fn untyped_scalars_keep_string_models_and_hexadecimal_numbers() {
    let doc = parse_yaml("scalar.yaml", "schema: 1\ncpu: {model: 80286}\nboard: {post_port: 0x80}\ncard: [{kind: uart-16550, port: 0x3F8, irq: none}]\nname: no\n").unwrap();
    assert_eq!(doc.cpu.unwrap().model, Some(CpuModel::I80286));
    assert_eq!(
        doc.board.unwrap().post_port,
        Some(NumberOrNone::Number(0x80))
    );
    assert_eq!(doc.card.unwrap()[0].port, Some(0x3F8));
    assert_eq!(doc.name.as_deref(), Some("no"));
}

#[test]
fn rtc_timestamps_require_quotes_including_anchor_definitions() {
    rejected(
        "schema: 1\nboard: {rtc: {start: 2000-01-01T00:00:00}}\n",
        "quoted string",
    );
    rejected(
        "schema: 1\nboard: {rtc: {start: !!timestamp 2000-01-01T00:00:00}}\n",
        "YAML tags are not allowed",
    );
    for value in ["\"2000-01-01T00:00:00\"", "'2000-01-01T00:00:00'"] {
        parse_yaml(
            "rtc.yaml",
            &format!("schema: 1\nboard: {{rtc: {{start: {value}}}}}\n"),
        )
        .unwrap();
    }
    rejected(
        "schema: 1\nname: &date 2000-01-01T00:00:00\nboard: {rtc: {start: *date}}\n",
        "quoted string",
    );
    parse_yaml(
        "rtc.yaml",
        "schema: 1\nname: &date '2000-01-01T00:00:00'\nboard: {rtc: {start: *date}}\n",
    )
    .unwrap();
}

#[test]
fn forbidden_fields_reject_null_in_every_table() {
    let timing = [
        "mhz",
        "frequency",
        "clock_hz",
        "cycles",
        "cycle_exact",
        "wait_states",
        "bus_mhz",
        "memory_wait_states",
        "timing",
    ];
    for prefix in ["", "board: ", "cpu: ", "card: ["] {
        for key in timing {
            let body = format!("{{{key}: null}}");
            let suffix = if prefix == "card: [" { "]" } else { "" };
            let input = if prefix.is_empty() {
                format!("schema: 1\n{key}: null\n")
            } else {
                format!("schema: 1\n{prefix}{body}{suffix}\n")
            };
            rejected(&input, "null is not allowed; omit the field instead");
        }
    }
    for (table, fields) in [
        ("", &["run", "presentation", "streams"][..]),
        (
            "board",
            &[
                "a20",
                "kbc",
                "keyboard",
                "keyboard_layout",
                "dip_switches",
                "chipset",
                "speaker",
            ][..],
        ),
        ("cpu", &["fpu", "turbo"][..]),
        ("memory", &["shadow", "umb", "upper"][..]),
        ("firmware", &["basic"][..]),
        ("card", &["monitor", "rom", "mmio"][..]),
        ("drive", &["media"][..]),
    ] {
        for key in fields {
            let field = format!("{{{key}: null}}");
            let body = match table {
                "" => format!("{key}: null"),
                "card" => format!("card: [{field}]"),
                "drive" => format!("card: [{{drives: [{field}]}}]"),
                _ => format!("{table}: {field}"),
            };
            rejected(
                &format!("schema: 1\n{body}\n"),
                "null is not allowed; omit the field instead",
            );
        }
    }
}

#[test]
fn null_is_rejected_in_every_value_position() {
    let fields = [
        ("card: [{kind: uart-16550, irq: VALUE}]", "irq"),
        ("card: [{kind: fdc-765, dma: VALUE}]", "dma"),
        ("card: [{kind: uart-16550, port: VALUE}]", "port"),
        ("board: {post_port: VALUE}", "post_port"),
        ("board: {rtc: {nvram: VALUE}}", "nvram"),
        ("board: VALUE", "board"),
        ("cpu: VALUE", "cpu"),
        ("firmware: {bios: {provides: VALUE}}", "provides"),
        ("card: VALUE", "card"),
        ("firmware: {option_roms: VALUE}", "option_roms"),
        ("card: [{drives: VALUE}]", "drives"),
        ("card: [VALUE]", "sequence item"),
        ("VALUE: ignored", "key"),
    ];
    for (body, field) in fields {
        for null in ["~", "null", "Null", "NULL", ""] {
            if null.is_empty() && field == "sequence item" {
                continue;
            }
            let input = format!("schema: 1\n{}\n", body.replace("VALUE", null));
            let error = parse_yaml("null.yaml", &input).expect_err(field);
            assert_eq!(
                error.message(),
                "null is not allowed; omit the field instead",
                "{field}: {error}"
            );
            assert_eq!(error.path(), std::path::Path::new("null.yaml"));
            assert_eq!(error.line_column().unwrap().0, 2, "{field}: {error}");
            assert!(error.line_column().unwrap().1 > 0);
        }
    }
    for input in [
        "schema: 1\nboard: # no board\n",
        "schema: 1\ncard:\n  - # no card\n",
        "schema: 1\nfirmware:\n  bios:\n    provides:\n      - # no service\n",
    ] {
        rejected(input, "null is not allowed; omit the field instead");
    }
    // These are strings, rather than YAML nulls.
    parse_yaml("strings.yaml", "schema: 1\nname: 'null'\ndescription: ''\n").unwrap();
}

#[test]
fn quoted_integers_are_rejected_at_every_integer_field_depth() {
    for body in [
        "board: {post_port: '128'}",
        "memory: {conventional_kib: '640'}",
        "memory: {extended_kib: '0'}",
        "firmware: {bios: {base: '983040'}}",
        "firmware: {option_roms: [{base: '819200'}]}",
        "card: [{port: '0x3F8'}]",
        "card: [{drives: [{unit: '0'}]}]",
        "card: [{drives: [{keep_generations: '7'}]}]",
        "card: [{drives: [{geometry: {cylinders: '615'}}]}]",
        "card: [{drives: [{geometry: {heads: '4'}}]}]",
        "card: [{drives: [{geometry: {sectors: '17'}}]}]",
    ] {
        let error = parse_yaml("integer.yaml", &format!("schema: 1\n{body}\n")).unwrap_err();
        assert!(error.message().contains("string"), "{error}");
        assert_eq!(error.line_column().unwrap().0, 2);
    }
}

#[test]
fn booleans_require_lowercase_unquoted_literals() {
    for value in ["True", "TRUE", "False", "FALSE", "'true'", "\"false\""] {
        for body in [
            format!("card: [{{remove: {value}}}]"),
            format!("card: [{{drives: [{{remove: {value}}}]}}]"),
        ] {
            assert!(
                parse_yaml("bool.yaml", &format!("schema: 1\n{body}\n")).is_err(),
                "{body}"
            );
        }
    }
}

#[test]
fn tags_are_rejected_on_scalars_mappings_and_binary_strings() {
    for body in [
        "name: !custom example",
        "board: !custom {model: ibm-5170}",
        "firmware: {bios: {sha256: !!binary YWJj}}",
        "name: !!str example",
        "name: !!binary YWJj",
        "board: !!map {model: ibm-5170}",
        "card: !!seq []",
        "name: ! example",
    ] {
        let error = parse_yaml("tags.yaml", &format!("schema: 1\n{body}\n")).unwrap_err();
        assert_eq!(error.message(), "YAML tags are not allowed", "{error}");
        assert_eq!(error.line_column(), Some((2, body.find('!').unwrap() + 1)));
    }
}

#[test]
fn schema_type_errors_point_at_the_value() {
    for value in ["\"1\"", "1.0", "~", "18446744073709551615"] {
        let error = parse_yaml("schema.yaml", &format!("schema: {value}\n")).unwrap_err();
        assert_eq!(error.line_column(), Some((1, 9)), "{error}");
    }
}

#[test]
fn a_bom_does_not_shift_a_quoted_rtc_start_on_line_one() {
    parse_yaml(
        "bom.yaml",
        "\u{feff}{schema: 1, board: {rtc: {start: '2000-01-01T00:00:00'}}}\n",
    )
    .unwrap();
    let error = parse_yaml("bom.yaml", "\u{feff}schema: \"1\"\n").unwrap_err();
    assert_eq!(error.line_column(), Some((1, 9)));
    assert_eq!(error.snippet(), Some("schema: \"1\"\n        ^"));
}

#[test]
fn numeric_looking_string_fields_preserve_their_unquoted_spelling() {
    let hash = "0".repeat(64);
    let file = parse_yaml("strings.yaml", &format!("schema: 1\nname: 01016\ncpu: {{model: 80286}}\nfirmware: {{bios: {{sha256: {hash}}}}}\n")).unwrap();
    assert_eq!(file.name.as_deref(), Some("01016"));
    assert_eq!(file.cpu.unwrap().model, Some(CpuModel::I80286));
    assert_eq!(
        file.firmware
            .unwrap()
            .bios
            .unwrap()
            .sha256
            .unwrap()
            .as_str(),
        hash
    );
}
