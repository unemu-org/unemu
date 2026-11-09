---
id: decision-3
title: Real-mode extension in x86jit
date: 2026-07-17
status: Accepted
deciders: Mikołaj Mikołajczyk
---

# decision-3 - Real-mode extension in x86jit

## Status

Accepted — 2026-07-17.

## Context

IBM PC/AT ([[decision-1]]) boots in real mode. x86jit today lifts only `Long64` /
`Compat32` (userspace style). Verified in the source, real mode is **not a small
addition but a rebuild of a hot path**:

- `CpuState` has no segment registers (CS/DS/ES/SS) — needed for every real-mode
  memory access, far jmp/call, push/pop, string ops with overrides.
- `Flags` does not model `IF` (nor `TF`/`IOPL`) — needed for `sti`/`cli`,
  `pushf`/`popf`, `iret`, interrupt gating.
- Addressing is baked flat (`host = base + guest_addr`); `segment:offset` with
  64K/`SP` wrap is new lifter arithmetic.
- `Exit::Exception` / `Exit::Syscall` exit to the embedder; in real mode `#DE` and
  every `INT n` must vector through the IVT **inside the guest** — mode-dependent.

`CpuState` is `#[repr(C)]` with offsets baked into `jit_abi.rs` and generated code.

## Decision

Add real mode in x86jit behind a new **`CpuMode::Real16`** (peer of `Long64` /
`Compat32`, using the existing `set_cpu_mode` extension point). Split into three
sub-seams with independent exit criteria:

- **(a)** segment registers in `CpuState` + 16-bit decode + straight-line code;
  oracle: SingleStepTests / Unicorn 16-bit.
- **(b)** `INT` / `IRET` / IVT + `IF` in `Flags` + exceptions vectored through the
  IVT (mode-dependent change to `Exit::Exception` / `Exit::Syscall`).
- **(c)** hardware IRQ injection — see [[decision-4]].

`CpuState` / `jit_abi` changes are **append-only** with an offset-assertion test
(ABI hazard for the engine's other modes). Merge back to main per [[decision-2]].

## Consequences

- (+) True PC boot; isolated from the engine's other modes by `CpuMode::Real16`.
- (−) Largest single work item; touches the hot flags/addressing paths.

## Trigger to revisit

If the append-only ABI constraint blocks a needed `CpuState` change, escalate a
layout decision (with a coordinated offset bump for the CPU core's other
consumers).
