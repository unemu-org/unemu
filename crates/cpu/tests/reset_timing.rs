// SPDX-License-Identifier: AGPL-3.0-or-later
//! Opt-in host timing for the 80286 reset path (core-contract.md §12.10).

use std::{hint::black_box, time::Instant};

use cpu::{x86jit_backend::RealSegments, Cpu, X86JitCpu};

#[test]
#[ignore = "host timing; run in release mode with --nocapture"]
fn reset_wall_times() {
    const RESETS: u32 = 200_000;
    let mut cpu = X86JitCpu::new_i286_bare(1 << 24, 0xF000, 0xFFF0, RealSegments::flat(0)).unwrap();
    for _ in 0..10_000 {
        black_box(&mut cpu).reset().unwrap();
    }
    let mut samples = Vec::new();
    for _ in 0..9 {
        let start = Instant::now();
        for _ in 0..RESETS {
            black_box(&mut cpu).reset().unwrap();
        }
        samples.push(start.elapsed().as_nanos() as f64 / f64::from(RESETS));
    }
    println!("reset ns/op: {samples:?}");
    samples.sort_by(f64::total_cmp);
    println!("reset median ns/op: {:.3}", samples[4]);
    assert_eq!(cpu.retired(), 0);
}
