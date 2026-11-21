// SPDX-License-Identifier: AGPL-3.0-or-later
//! `unemu run --machine --trace-exceptions` records the exception and
//! interrupt deliveries and writes the last entries to stderr when the run
//! ends, consecutive identical deliveries folded into one, with a summary,
//! and `--diag-at N` adds the last deliveries to the diagnostic report
//! (commands.md, Build / run / test; `backlog/docs/design/observation.md`
//! §Exception and interrupt deliveries). The system ROMs are the project's
//! own bytes from `assembly::test_support`: `deliveries_rom` takes a #UD, a
//! software interrupt, an IRQ0 tick and a protected-mode #GP;
//! `delivery_overrun_rom` faults in a loop until its stack overruns;
//! `delivery_ring_rom` takes 12,000 software interrupts that never fold.

use std::ffi::OsString;

fn run(rom: &[u8], extra: &[&str]) -> (u8, String) {
    let temp = machine_file::test_support::TempDir::new("exception-trace");
    std::fs::write(temp.path().join("rom.bin"), rom).unwrap();
    let machine = temp.path().join("rom.toml");
    std::fs::write(
        &machine,
        format!(
            "schema = 1\n[board]\nmodel = \"ibm-5170\"\n[firmware.bios]\n\
             image = \"rom.bin\"\nsha256 = \"{}\"\n[[card]]\nkind = \"mda\"\n",
            assembly::test_support::fixture_sha256(rom)
        ),
    )
    .unwrap();
    let mut args: Vec<OsString> = vec![
        "run".into(),
        "--machine".into(),
        machine.into_os_string(),
        "--allow-unlisted-firmware".into(),
        "--no-store".into(),
    ];
    args.extend(extra.iter().map(OsString::from));
    let (mut output, mut errors) = (Vec::new(), Vec::new());
    let code = cli_pc::command::run(&args, &mut output, &mut errors);
    (code, String::from_utf8(errors).unwrap())
}

/// The lines a trace writes, without the unlisted-firmware warning.
fn trace_lines(errors: &str) -> Vec<&str> {
    errors
        .lines()
        .filter(|line| !line.starts_with("warning: running unlisted firmware"))
        .collect()
}

/// The recording's two header lines, checked, then the rest.
fn after_header<'a>(lines: &'a [&'a str], entries: usize) -> &'a [&'a str] {
    assert!(
        lines[0].starts_with(
            "trace    exceptions and interrupts the guest took from retired 0 to retired "
        ),
        "{}",
        lines[0]
    );
    assert_eq!(
        lines[1],
        format!(
            "trace    the last {entries} entries, oldest first; an entry with a range and xN stands for N identical deliveries in a row"
        )
    );
    &lines[2..]
}

/// One delivery of each kind, each with its retired count, saved CS:IP and
/// the bytes there: the #UD saves the invalid opcode itself, INT 60h the
/// instruction after it, IRQ0 the CLI after the HLT it ended, and the #GP
/// the faulting MOV DS,AX in protected mode with its error code. Without
/// the switch the run stops the same way with no trace.
#[test]
fn the_trace_of_one_delivery_of_each_kind_is_exact() {
    let rom = assembly::test_support::deliveries_rom();
    let (code, errors) = run(&rom, &["--trace-exceptions"]);
    assert_eq!(code, 0, "{errors}");
    let lines = trace_lines(&errors);
    assert_eq!(
        after_header(&lines, 4),
        [
            "delivery 12 fault #UD vector 06 at f800:002e linear 0f802e error - cpl 0->0 bytes 0f ff cd 60 b0 11 e6 20 ax 0000 bx 0000 cx 0000 dx 0000 si 0000 di 0000 bp 0000 ds 0000 es 0000 ss:sp 0000:7000 flags 0046",
            "delivery 18 software INT 60h vector 60 at f800:0032 linear 0f8032 error - cpl 0->0 bytes b0 11 e6 20 b0 08 e6 21 ax 0000 bx 0000 cx 0000 dx 0000 si 0000 di 0000 bp 0000 ds 0000 es 0000 ss:sp 0000:7000 flags 0046",
            "delivery 37 irq IRQ 0 vector 08 at f800:0054 linear 0f8054 error - cpl 0->0 bytes fa b0 ff e6 21 2e 0f 01 ax 0010 bx 0000 cx 0000 dx 0000 si 0000 di 0000 bp 0000 ds 0000 es 0000 ss:sp 0000:7000 flags 0246",
            "delivery 54 fault #GP vector 0d at 0008:007b linear 0f807b error 00f8 cpl 0->0 bytes 8e d8 f4 55 89 e5 83 46 ax 00f8 bx 0000 cx 0000 dx 0000 si 0000 di 0000 bp 0000 ds 0000 es 0000 ss:sp 0010:7000 flags 0046",
            "summary  4 deliveries, 4 of them in the entries kept, 0 dropped before they were read",
            "summary  #UD: 1",
            "summary  #GP: 1",
            "summary  IRQ 0: 1",
            "summary  INT 60h: 1",
            "stopped: halt",
        ]
    );
    let (plain_code, plain) = run(&rom, &[]);
    assert_eq!(plain_code, code);
    assert_eq!(trace_lines(&plain), ["stopped: halt"]);
}

/// The report taken at step 500, while the guest waits for IRQ0, lists the
/// two deliveries so far, oldest first, with the counts.
#[test]
fn diag_at_lists_the_last_deliveries() {
    let (code, errors) = run(
        &assembly::test_support::deliveries_rom(),
        &["--diag-at", "500"],
    );
    assert_eq!(code, 0, "{errors}");
    assert!(
        errors.contains(
            "\ndelivery the last 2 of 2 exceptions and interrupts in 2 entries, oldest first (0 dropped before they were read)\n\
             delivery 12 fault #UD vector 06 at f800:002e linear 0f802e error - cpl 0->0 bytes 0f ff cd 60 b0 11 e6 20 ax 0000 bx 0000 cx 0000 dx 0000 si 0000 di 0000 bp 0000 ds 0000 es 0000 ss:sp 0000:7000 flags 0046\n\
             delivery 18 software INT 60h vector 60 at f800:0032 linear 0f8032 error - cpl 0->0 bytes b0 11 e6 20 b0 08 e6 21 ax 0000 bx 0000 cx 0000 dx 0000 si 0000 di 0000 bp 0000 ds 0000 es 0000 ss:sp 0000:7000 flags 0046\n\
             pic      master "
        ),
        "{errors}"
    );
}

/// A guest that faults in a loop until its stack overruns and the CPU
/// shuts down: the first #UD, then the 10,921 at the handler folded into
/// one entry, and the summary of all 10,922. Its range `12..12` is a run at
/// one retired count: each delivery faults again before an instruction
/// retires; the run goes on through the
/// reset to the halt exactly as without the trace.
#[test]
fn a_fault_loop_folds_into_one_entry() {
    let rom = assembly::test_support::delivery_overrun_rom();
    let (code, errors) = run(&rom, &["--trace-exceptions"]);
    assert_eq!(code, 0, "{errors}");
    let lines = trace_lines(&errors);
    assert_eq!(
        after_header(&lines, 2),
        [
            "delivery 12 fault #UD vector 06 at f800:0024 linear 0f8024 error - cpl 0->0 bytes 0f ff f4 00 00 00 00 00 ax 1000 bx 0000 cx 0000 dx 0000 si 0000 di 0000 bp 0000 ds 0000 es 0000 ss:sp 1000:ffff flags 0097",
            "delivery 12..12 x10921 fault #UD vector 06 at f800:0040 linear 0f8040 error - cpl 0->0 bytes 0f ff 00 00 00 00 00 00 ax 1000 bx 0000 cx 0000 dx 0000 si 0000 di 0000 bp 0000 ds 0000 es 0000 ss:sp 1000:0009 flags 0097",
            "summary  10922 deliveries, 10922 of them in the entries kept, 0 dropped before they were read",
            "summary  #UD: 10922",
            "stopped: halt",
        ]
    );
    let (plain_code, plain) = run(&rom, &[]);
    assert_eq!(plain_code, code);
    assert_eq!(trace_lines(&plain), ["stopped: halt"]);
}

/// 12,000 deliveries that never fold: the recording keeps exactly the
/// last 10,000, oldest first, each at its retired count, and the summary
/// counts all 12,000, 6,000 per vector.
#[test]
fn the_recording_keeps_the_last_entries() {
    let rom = assembly::test_support::delivery_ring_rom();
    let (code, errors) = run(&rom, &["--trace-exceptions"]);
    assert_eq!(code, 0, "{errors}");
    let lines = trace_lines(&errors);
    let rest = after_header(&lines, 10_000);
    for (index, line) in rest[..10_000].iter().enumerate() {
        let n = 2_000 + index as u64;
        let (vector, ip) = if n.is_multiple_of(2) {
            (0x60, 0x27)
        } else {
            (0x61, 0x29)
        };
        let retired = 12 + 5 * (n / 2) + 2 * (n % 2);
        assert!(
            line.starts_with(&format!(
                "delivery {retired} software INT {vector:02X}h vector {vector:02x} at f800:{ip:04x} "
            )),
            "{index}: {line}"
        );
    }
    assert_eq!(
        rest[10_000..],
        [
            "summary  12000 deliveries, 10000 of them in the entries kept, 0 dropped before they were read",
            "summary  INT 60h: 6000",
            "summary  INT 61h: 6000",
            "stopped: halt",
        ]
    );
}

/// A guest stuck delivering #GP stops with the delivery-stall error; the
/// deliveries of the slice that failed are still recorded, the first at
/// the load and every other at the handler, folded, before the stop line
/// and the CPU stop report.
#[test]
fn a_delivery_stall_is_traced_up_to_its_stop() {
    let rom = assembly::test_support::delivery_stall_rom();
    let (code, errors) = run(&rom, &["--trace-exceptions"]);
    assert_eq!(code, 1, "{errors}");
    let lines = trace_lines(&errors);
    let rest = after_header(&lines, 2);
    assert!(
        rest[0].starts_with("delivery 11 fault #GP vector 0d at 0008:0023 "),
        "{}",
        rest[0]
    );
    assert!(
        rest[1].starts_with("delivery 11..11 x11263 fault #GP vector 0d at 0008:0040 "),
        "{}",
        rest[1]
    );
    assert_eq!(
        rest[2..4],
        [
            "summary  11264 deliveries, 11264 of them in the entries kept, 0 dropped before they were read",
            "summary  #GP: 11264",
        ]
    );
    assert!(rest[4].starts_with("stopped: "), "{}", rest[4]);
    let (plain_code, plain) = run(&rom, &[]);
    assert_eq!(plain_code, code);
    assert_eq!(trace_lines(&plain), rest[4..]);
}
