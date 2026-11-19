// SPDX-License-Identifier: AGPL-3.0-or-later
//! `upcli run --machine --trace-exceptions` writes a line per exception and
//! interrupt delivery to stderr, bounded, with a summary at the end, and
//! `--diag-at N` adds the last deliveries to the diagnostic report
//! (commands.md, Build / run / test; `backlog/docs/design/observation.md`
//! §Exception and interrupt deliveries). The system ROMs are the project's
//! own bytes from `assembly::test_support`: `deliveries_rom` takes a #UD, a
//! software interrupt, an IRQ0 tick and a protected-mode #GP;
//! `delivery_overrun_rom` faults in a loop until its stack overruns.

use std::ffi::OsString;

fn run(rom: &[u8], extra: &[&str]) -> (u8, String) {
    let temp = assembly::test_support::TempDir::new("exception-trace");
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
    assert_eq!(
        trace_lines(&errors),
        [
            "delivery 12 fault #UD vector 06 at f800:002e linear 0f802e error - cpl 0->0 bytes 0f ff cd 60 b0 11 e6 20",
            "delivery 18 software INT 60h vector 60 at f800:0032 linear 0f8032 error - cpl 0->0 bytes b0 11 e6 20 b0 08 e6 21",
            "delivery 37 irq IRQ 0 vector 08 at f800:0054 linear 0f8054 error - cpl 0->0 bytes fa b0 ff e6 21 2e 0f 01",
            "delivery 54 fault #GP vector 0d at 0008:007b linear 0f807b error 00f8 cpl 0->0 bytes 8e d8 f4 55 89 e5 83 46",
            "summary  4 deliveries, 4 lines written, 0 dropped before they were read",
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
            "\ndelivery the last 2 of 2 exceptions and interrupts, oldest first (0 dropped before they were read)\n\
             delivery 12 fault #UD vector 06 at f800:002e linear 0f802e error - cpl 0->0 bytes 0f ff cd 60 b0 11 e6 20\n\
             delivery 18 software INT 60h vector 60 at f800:0032 linear 0f8032 error - cpl 0->0 bytes b0 11 e6 20 b0 08 e6 21\n\
             pic      master "
        ),
        "{errors}"
    );
}

/// A guest that faults in a loop until its stack overruns and the CPU
/// shuts down writes the first 10,000 lines, the note that lines stop, and
/// the summary of all 10,922 #UDs; the run goes on through the reset to
/// the halt exactly as without the trace.
#[test]
fn a_fault_loop_writes_the_bound_and_a_summary() {
    let rom = assembly::test_support::delivery_overrun_rom();
    let (code, errors) = run(&rom, &["--trace-exceptions"]);
    assert_eq!(code, 0, "{errors}");
    let lines = trace_lines(&errors);
    assert_eq!(lines.len(), 10_000 + 4, "{:?}", &lines[10_000..]);
    assert_eq!(
        lines[0],
        "delivery 12 fault #UD vector 06 at f800:0024 linear 0f8024 error - cpl 0->0 bytes 0f ff f4 00 00 00 00 00"
    );
    for line in &lines[1..10_000] {
        assert_eq!(
            *line,
            "delivery 12 fault #UD vector 06 at f800:0040 linear 0f8040 error - cpl 0->0 bytes 0f ff 00 00 00 00 00 00"
        );
    }
    assert_eq!(
        &lines[10_000..],
        [
            "trace    10000 delivery lines written; from here a count line every 1000000 deliveries",
            "summary  10922 deliveries, 10000 lines written, 0 dropped before they were read",
            "summary  #UD: 10922",
            "stopped: halt",
        ]
    );
    let (plain_code, plain) = run(&rom, &[]);
    assert_eq!(plain_code, code);
    assert_eq!(trace_lines(&plain), ["stopped: halt"]);
}

/// A guest stuck delivering #GP stops with the delivery-stall error; the
/// deliveries of the slice that failed are still traced, bounded, before
/// the stop line and the CPU stop report.
#[test]
fn a_delivery_stall_is_traced_up_to_its_stop() {
    let rom = assembly::test_support::delivery_stall_rom();
    let (code, errors) = run(&rom, &["--trace-exceptions"]);
    assert_eq!(code, 1, "{errors}");
    let lines = trace_lines(&errors);
    assert!(lines[..10_000]
        .iter()
        .all(|line| line.starts_with("delivery 11 fault #GP vector 0d at 0008:00")));
    assert_eq!(
        &lines[10_000..10_003],
        [
            "trace    10000 delivery lines written; from here a count line every 1000000 deliveries",
            "summary  11264 deliveries, 10000 lines written, 0 dropped before they were read",
            "summary  #GP: 11264",
        ]
    );
    assert!(lines[10_003].starts_with("stopped: "), "{}", lines[10_003]);
    let (plain_code, plain) = run(&rom, &[]);
    assert_eq!(plain_code, code);
    assert_eq!(trace_lines(&plain), lines[10_003..]);
}
